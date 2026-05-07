use std::sync::Arc;

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::k8s::source::GitRepository;
use crate::k8s::watcher::GitRepoStore;
use crate::ui::theme;

const DIM: Style = Style::new().fg(Color::Rgb(120, 125, 145));

/// Look up a GitRepository from the reflector store by its sourceRef.
/// `ref_namespace` is the optional namespace from sourceRef; if absent,
/// the referent's namespace defaults to `fallback_namespace` (the
/// namespace of the resource holding the reference).
pub fn find_gitrepo(
    store: &GitRepoStore,
    ref_namespace: Option<&str>,
    fallback_namespace: &str,
    name: &str,
) -> Option<Arc<GitRepository>> {
    let ns = ref_namespace.unwrap_or(fallback_namespace);
    store
        .state()
        .iter()
        .find(|gr| {
            gr.metadata.namespace.as_deref() == Some(ns)
                && gr.metadata.name.as_deref() == Some(name)
        })
        .cloned()
}

/// Render a "Source:" line with health icon and revision (or reason)
/// when the source is a GitRepository we can look up. Falls back to a
/// plain `kind/name` line when it isn't a GR or the store isn't synced.
pub fn source_line<'a>(
    label: &'static str,
    label_style: Style,
    source_kind: &str,
    source_name: &str,
    gr: Option<&GitRepository>,
    gr_synced: bool,
) -> Line<'a> {
    let id = format!("{source_kind}/{source_name}");

    if source_kind != "GitRepository" {
        return Line::from(vec![Span::styled(label, label_style), Span::raw(id)]);
    }

    match gr {
        Some(gr) => {
            let ready = gr
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"));
            let (icon, icon_style, suffix, suffix_style) = match ready {
                Some(c) if c.status == "True" => {
                    let rev = gr
                        .status
                        .as_ref()
                        .and_then(|s| s.artifact.as_ref())
                        .and_then(|a| a.revision.as_deref())
                        .map(short_revision)
                        .unwrap_or_else(|| "ready".to_string());
                    ("✓ ", theme::STATUS_READY, rev, DIM)
                }
                Some(c) => {
                    let reason = if c.message.is_empty() {
                        c.reason.clone()
                    } else {
                        c.message.clone()
                    };
                    ("✗ ", theme::STATUS_NOT_READY, reason, DIM)
                }
                None => ("? ", theme::STATUS_UNKNOWN, "no conditions".to_string(), DIM),
            };
            Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(icon, icon_style),
                Span::raw(format!("{id}  ")),
                Span::styled(suffix, suffix_style),
            ])
        }
        None if gr_synced => Line::from(vec![
            Span::styled(label, label_style),
            Span::styled("✗ ", theme::STATUS_NOT_READY),
            Span::raw(format!("{id}  ")),
            Span::styled("not found".to_string(), theme::STATUS_NOT_READY),
        ]),
        None => Line::from(vec![Span::styled(label, label_style), Span::raw(id)]),
    }
}

/// Shorten a Flux artifact revision like `8.22.0@sha1:50e15fa09734...`
/// to `8.22.0@50e15fa`. Passes through values that don't match the
/// `<tag>@<algo>:<hash>` shape.
pub(crate) fn short_revision(full: &str) -> String {
    if let Some((tag, rest)) = full.split_once('@') {
        let hash = rest.split(':').next_back().unwrap_or(rest);
        let short_len = hash.len().min(7);
        let mut end = short_len;
        while end > 0 && !hash.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}@{}", tag, &hash[..end])
    } else {
        full.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_sha_after_at() {
        assert_eq!(
            short_revision("8.19.0@sha1:bbb7041bcaceb908"),
            "8.19.0@bbb7041"
        );
    }

    #[test]
    fn passes_through_without_at() {
        assert_eq!(short_revision("just-a-tag"), "just-a-tag");
    }

    #[test]
    fn handles_short_hash_without_truncation() {
        assert_eq!(short_revision("8.19.0@abc"), "8.19.0@abc");
    }

    #[test]
    fn does_not_split_multibyte_chars_when_truncating() {
        // 7 bytes of "héllo" lands mid char-boundary; trimmer must back off.
        // ("héllo" is 6 bytes, so pad it to 8+ with multibyte chars.)
        let rev = short_revision("v1@héllo🌱world");
        assert!(
            rev.starts_with("v1@"),
            "tag prefix must survive: {rev}"
        );
        // Body must still be valid UTF-8 (just compiling/asserting len works).
        assert!(rev.is_char_boundary(rev.len()));
    }
}
