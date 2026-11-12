use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use tracing::{debug, warn};
use unic_langid::LanguageIdentifier;

use crate::settings::Language;

const ENGLISH: &str = include_str!("../locales/en.ftl");
const POLISH: &str = include_str!("../locales/pl.ftl");

/// Interface strings in English and Polish, loaded from the embedded Fluent files.
pub(crate) struct I18n {
    english: FluentBundle<FluentResource>,
    polish: FluentBundle<FluentResource>,
    language: Language,
}

impl std::fmt::Debug for I18n {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("I18n")
            .field("language", &self.language)
            .finish_non_exhaustive()
    }
}

impl I18n {
    pub(crate) fn new(language: Language) -> Self {
        Self {
            english: bundle("en-US", ENGLISH),
            polish: bundle("pl-PL", POLISH),
            language,
        }
    }

    pub(crate) fn set_language(&mut self, language: Language) {
        self.language = language;
    }

    /// The message `key` in the current language, falling back to English and then to the key itself.
    pub(crate) fn tr(&self, key: &str) -> String {
        self.format(key, None)
    }

    /// Like [`I18n::tr`], but returns `fallback` without a warning when no language defines `key`. For texts that
    /// come from `rustium-core` (node and socket names) and only some of which are translated.
    pub(crate) fn tr_or(&self, key: &str, fallback: &str) -> String {
        let primary = match self.language {
            Language::English => &self.english,
            Language::Polish => &self.polish,
        };
        if primary.has_message(key) || self.english.has_message(key) {
            self.format(key, None)
        } else {
            fallback.to_owned()
        }
    }

    /// Like [`I18n::tr`] with Fluent arguments such as `{ $slot }`.
    pub(crate) fn tr_args(&self, key: &str, args: &[(&str, FluentValue<'_>)]) -> String {
        let mut fluent_args = FluentArgs::new();
        for (name, value) in args {
            fluent_args.set(*name, value.clone());
        }
        self.format(key, Some(&fluent_args))
    }

    fn format(&self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        let primary = match self.language {
            Language::English => &self.english,
            Language::Polish => &self.polish,
        };
        for bundle in [primary, &self.english] {
            if let Some(pattern) = bundle.get_message(key).and_then(|message| message.value()) {
                let mut errors = Vec::new();
                let text = bundle.format_pattern(pattern, args, &mut errors);
                if !errors.is_empty() {
                    debug!(key, ?errors, "message formatted with errors");
                }
                return text.into_owned();
            }
        }
        warn!(key, "missing interface message");
        key.to_owned()
    }
}

fn bundle(locale: &str, source: &str) -> FluentBundle<FluentResource> {
    let language: LanguageIdentifier = locale.parse().unwrap_or_default();
    let mut bundle = FluentBundle::new(vec![language]);
    bundle.set_use_isolating(false);
    let resource = FluentResource::try_new(source.to_owned()).unwrap_or_else(|(resource, errors)| {
        warn!(locale, ?errors, "locale file has syntax errors");
        resource
    });
    if let Err(errors) = bundle.add_resource(resource) {
        warn!(locale, ?errors, "locale file has duplicate messages");
    }
    bundle
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use fluent_bundle::FluentResource;
    use fluent_syntax::ast::Entry;

    use super::*;

    fn message_ids(source: &str) -> BTreeSet<String> {
        let resource = FluentResource::try_new(source.to_owned()).unwrap();
        resource
            .entries()
            .filter_map(|entry| match entry {
                Entry::Message(message) => Some(message.id.name.to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn polish_and_english_define_the_same_messages() {
        assert_eq!(message_ids(POLISH), message_ids(ENGLISH));
    }

    #[test]
    fn arguments_are_substituted() {
        let i18n = I18n::new(Language::Polish);
        let text = i18n.tr_args("palette-hint", &[("slot", FluentValue::from(3))]);
        assert_eq!(text, "Klik: wstaw do slotu 3 · prawy przycisk: edytuj");
    }
}
