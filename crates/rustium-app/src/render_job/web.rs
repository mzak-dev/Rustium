//! Render jobs in the browser: there are none.
//!
//! A job renders on a worker thread with a tracer of its own and writes files or pipes frames into `ffmpeg`; a
//! browser offers none of that. The interface keeps its `Option<JobHandle>`, which here can only be `None`, so the
//! code that shows a running job compiles unchanged and never runs.

use crate::render_output::SaveOutcome;

/// A running job; it has no values in the browser.
#[derive(Debug)]
pub(crate) enum JobHandle {}

impl JobHandle {
    pub(crate) fn fraction(&self) -> f32 {
        match *self {}
    }

    pub(crate) fn frames(&self) -> (u32, u32) {
        match *self {}
    }

    pub(crate) fn cancel(&self) {
        match *self {}
    }

    pub(crate) fn poll(&self) -> Option<SaveOutcome> {
        match *self {}
    }
}
