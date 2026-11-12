#![expect(clippy::unwrap_used, reason = "integration tests fail by panicking")]
//! The server end to end without a window: HTTP client → server thread → channel → fake UI thread → reply.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use rustium_mcp::{Permissions, Request, Server, SharedPermissions, channel, run_stdio_bridge};
use serde_json::{Value, json};

struct Fixture {
    server: Server,
    permissions: SharedPermissions,
    wakes: Arc<AtomicUsize>,
    ui: thread::JoinHandle<Vec<Request>>,
}

/// Starts a server whose "UI thread" answers every request with `{"echo": <debug of request>}` and returns
/// the requests it saw when the server is dropped.
fn start() -> Fixture {
    let permissions = SharedPermissions::new(Permissions::default());
    let wakes = Arc::new(AtomicUsize::new(0));
    let waker = {
        let wakes = Arc::clone(&wakes);
        Arc::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
        })
    };
    let (bridge, requests) = channel(permissions.clone(), waker);
    let ui = thread::spawn(move || {
        let mut seen = Vec::new();
        while let Ok(envelope) = requests.recv() {
            let reply = match &envelope.request {
                Request::StartRender => Err("not available until M3".to_owned()),
                other => Ok(json!({"echo": format!("{other:?}")})),
            };
            seen.push(envelope.request);
            let _ = envelope.reply.send(reply);
        }
        seen
    });
    Fixture {
        server: Server::start(0, bridge).unwrap(),
        permissions,
        wakes,
        ui,
    }
}

fn post(url: &str, body: &Value) -> Value {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut response = agent
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .send(body.to_string())
        .unwrap();
    let text = response.body_mut().read_to_string().unwrap();
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"))
}

fn call(url: &str, id: u32, tool: &str, arguments: &Value) -> Value {
    post(
        url,
        &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool, "arguments": arguments}}),
    )
}

#[test]
fn a_client_lists_tools_and_reads_the_model_through_the_ui_thread() {
    let fixture = start();
    let url = fixture.server.url();

    let initialize = post(
        &url,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
    );
    assert!(
        initialize["result"]["capabilities"]["tools"].is_object(),
        "{initialize}"
    );

    let list = post(&url, &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    for expected in [
        "scene_info",
        "get_voxels",
        "get_region",
        "get_palette",
        "set_voxels",
        "erase_voxels",
        "apply_brush",
        "set_palette_color",
        "run_script",
        "start_render",
        "get_materials",
        "get_material_node_kinds",
        "create_material",
        "duplicate_material",
        "delete_material",
        "rename_material",
        "set_material_casts_shadow",
        "set_material_surface",
        "edit_material_graph",
        "assign_material",
    ] {
        assert!(names.contains(&expected), "{expected} missing from {names:?}");
    }

    let reply = call(&url, 3, "scene_info", &json!({}));
    assert_eq!(reply["result"]["isError"], json!(false), "{reply}");
    assert!(
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("SceneInfo"),
        "{reply}"
    );

    let reply = call(
        &url,
        4,
        "set_voxels",
        &json!({"voxels": [{"x": 1, "y": 2, "z": 3, "color": 5}, {"x": 0, "y": 0, "z": 0}]}),
    );
    assert!(
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("SetVoxels"),
        "{reply}"
    );

    // The retired `material` key must be an error, not a silent erase.
    let reply = call(
        &url,
        5,
        "set_voxels",
        &json!({"voxels": [{"x": 1, "y": 2, "z": 3, "material": 5}]}),
    );
    assert!(
        reply["result"]["isError"] == json!(true) || reply.get("error").is_some(),
        "{reply}"
    );

    let wakes = fixture.wakes.load(Ordering::SeqCst);
    assert_eq!(wakes, 2, "one wake per request that reached the UI thread");
    drop(fixture.server);
    let seen = fixture.ui.join().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(matches!(&seen[1], Request::SetVoxels { voxels, .. } if voxels.len() == 2 && voxels[1].color.is_none()));
}

#[test]
fn disabled_permissions_are_refused_with_a_clear_error_and_never_reach_the_ui_thread() {
    let fixture = start();
    let url = fixture.server.url();
    let reply = call(&url, 1, "run_script", &json!({"source": "scene.set(0,0,0,1)"}));
    assert_eq!(reply["result"]["isError"], json!(true), "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Run scripts") && text.contains("turned off"), "{text}");

    let mut permissions = Permissions {
        edit_voxels: false,
        ..Permissions::default()
    };
    fixture.permissions.set(permissions);
    let reply = call(&url, 2, "erase_voxels", &json!({"positions": [[0, 0, 0]]}));
    assert_eq!(reply["result"]["isError"], json!(true), "{reply}");
    assert!(
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Edit voxels")
    );

    permissions.run_scripts = true;
    permissions.edit_voxels = true;
    fixture.permissions.set(permissions);
    let reply = call(&url, 3, "run_script", &json!({"source": "print(1)"}));
    assert_eq!(reply["result"]["isError"], json!(false), "{reply}");

    drop(fixture.server);
    assert_eq!(fixture.ui.join().unwrap().len(), 1, "only the allowed call arrived");
}

#[test]
fn errors_reported_by_the_ui_thread_become_tool_errors() {
    let permissions = Permissions {
        start_renders: true,
        ..Permissions::default()
    };
    let fixture = start();
    fixture.permissions.set(permissions);
    let reply = call(&fixture.server.url(), 1, "start_render", &json!({}));
    assert_eq!(reply["result"]["isError"], json!(true), "{reply}");
    assert!(reply["result"]["content"][0]["text"].as_str().unwrap().contains("M3"));
}

#[test]
fn the_stdio_bridge_forwards_to_the_running_server() {
    let fixture = start();
    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "get_palette", "arguments": {}}}),
    ]
    .map(|message| message.to_string())
    .join("\n");
    let mut output = Vec::new();
    run_stdio_bridge(&fixture.server.url(), input.as_bytes(), &mut output).unwrap();
    let text = String::from_utf8(output).unwrap();
    let answers: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(answers.len(), 2, "{text}");
    assert_eq!(answers[0]["id"], 1);
    assert!(
        answers[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("GetPalette"),
        "{text}"
    );
}

#[test]
fn a_busy_port_is_reported_as_a_bind_error() {
    let fixture = start();
    let port = fixture.server.address().port();
    let (bridge, _requests) = channel(SharedPermissions::new(Permissions::default()), Arc::new(|| {}));
    let error = Server::start(port, bridge).unwrap_err();
    assert!(error.to_string().contains(&port.to_string()), "{error}");
}

#[test]
fn web_pages_and_foreign_hosts_are_refused() {
    let fixture = start();
    let url = fixture.server.url();
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let body =
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "scene_info", "arguments": {}}});
    let send = |header: &str, value: &str| {
        agent
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header(header, value)
            .send(body.to_string())
            .unwrap()
            .status()
    };
    assert_eq!(send("Origin", "https://evil.example"), 403);
    assert_eq!(send("Host", "evil.example"), 403);
    drop(fixture.server);
    assert!(fixture.ui.join().unwrap().is_empty(), "nothing reached the UI thread");
}
