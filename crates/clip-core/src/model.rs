//! The universal content model: everything that can land on the clipboard maps
//! to one [`ContentKind`]. This is the heart of the "if it can be copied, Clip
//! keeps it" promise.

use serde::{Deserialize, Serialize};

/// Top-level classification of a captured clipboard item.
///
/// The OS clipboard exposes several *representations* of a single copy (e.g. an
/// image copied from a browser also carries HTML and a URL). We pick the richest
/// meaningful one to drive the UI, while the raw representations are preserved in
/// storage so paste-back is lossless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    /// Plain text with no richer interpretation.
    Text,
    /// A web/URL link.
    Link,
    /// A CSS/hex/rgb color value.
    Color,
    /// Source code (heuristically detected).
    Code,
    /// HTML / RTF rich text.
    RichText,
    /// A raster image, including screenshots.
    Image,
    /// One or more file/folder paths.
    Files,
    /// A video file reference or blob.
    Video,
    /// An audio file reference or blob.
    Audio,
    /// Anything else — stored as raw bytes with a MIME tag.
    Binary,
}

impl ContentKind {
    /// Stable string used as the storage discriminator and FTS facet.
    pub fn as_str(self) -> &'static str {
        match self {
            ContentKind::Text => "text",
            ContentKind::Link => "link",
            ContentKind::Color => "color",
            ContentKind::Code => "code",
            ContentKind::RichText => "rich_text",
            ContentKind::Image => "image",
            ContentKind::Files => "files",
            ContentKind::Video => "video",
            ContentKind::Audio => "audio",
            ContentKind::Binary => "binary",
        }
    }

    /// Parse from the stable storage discriminator produced by [`Self::as_str`].
    /// (Named `from_token` rather than `from_str` to avoid clashing with the
    /// `FromStr` trait, which would imply different fallible-parse semantics.)
    pub fn from_token(s: &str) -> Option<ContentKind> {
        Some(match s {
            "text" => ContentKind::Text,
            "link" => ContentKind::Link,
            "color" => ContentKind::Color,
            "code" => ContentKind::Code,
            "rich_text" => ContentKind::RichText,
            "image" => ContentKind::Image,
            "files" => ContentKind::Files,
            "video" => ContentKind::Video,
            "audio" => ContentKind::Audio,
            "binary" => ContentKind::Binary,
            _ => return None,
        })
    }

    /// Whether items of this kind are typically large enough to store
    /// by-reference (path + metadata + preview) rather than inlining the bytes.
    pub fn prefers_reference(self) -> bool {
        matches!(
            self,
            ContentKind::Files | ContentKind::Video | ContentKind::Audio
        )
    }
}

/// A captured clipboard item as persisted in the local store and synced.
///
/// `body` holds inline content for small/text items; large items keep a
/// `blob_ref` (content-addressed key into on-disk/object storage) instead. Both
/// can be set (e.g. an image with a thumbnail preview inline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    /// Content hash (blake3 hex) — also the dedup key and content address.
    pub id: String,
    pub kind: ContentKind,
    /// Short, human-facing preview/summary (e.g. the text, the filename list).
    pub preview: String,
    /// Inline content bytes for small items (UTF-8 text, thumbnail, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Vec<u8>>,
    /// Content-addressed reference for large blobs stored outside the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_ref: Option<String>,
    /// HTML representation of a rich-text copy, so **formatting survives** paste
    /// on other devices. `None` for plain content. The plain text stays in
    /// `body`, so a plain paste is always available too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// Byte size of the full content (not just the preview).
    pub size: u64,
    /// Unix millis when first captured.
    pub created_at: i64,
    pub pinned: bool,
}

/// A freshly observed clipboard item before it is assigned timestamps/flags.
/// Produced by the capture layer, consumed by the store (which dedups by `id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewClip {
    pub kind: ContentKind,
    pub preview: String,
    pub body: Option<Vec<u8>>,
    pub blob_ref: Option<String>,
    /// HTML for rich-text copies (see [`Clip::html`]); `None` for plain content.
    pub html: Option<String>,
    pub size: u64,
    /// Precomputed dedup id. Set when the natural identity is *not* the inline
    /// body — e.g. an image whose `body` holds only a thumbnail but which must
    /// dedup on the full-resolution content. `None` ⇒ derive from the payload.
    pub id: Option<String>,
}

impl NewClip {
    /// Compute the dedup id (content hash) for this item.
    ///
    /// If an explicit [`Self::id`] was set it wins; otherwise hash the kind
    /// plus whichever payload identifies the content: inline body, blob
    /// reference, or — failing both — the preview text.
    pub fn dedup_id(&self) -> String {
        if let Some(id) = &self.id {
            return id.clone();
        }
        let payload: &[u8] = self
            .body
            .as_deref()
            .or_else(|| self.blob_ref.as_deref().map(str::as_bytes))
            .unwrap_or(self.preview.as_bytes());
        crate::hash::content_hash(self.kind, payload)
    }

    pub fn into_clip(self, created_at: i64) -> Clip {
        Clip {
            id: self.dedup_id(),
            kind: self.kind,
            preview: self.preview,
            body: self.body,
            blob_ref: self.blob_ref,
            html: self.html,
            size: self.size,
            created_at,
            pinned: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_strings_are_stable() {
        assert_eq!(ContentKind::Image.as_str(), "image");
        assert_eq!(ContentKind::Files.as_str(), "files");
    }

    #[test]
    fn kind_roundtrips_through_str() {
        for k in [
            ContentKind::Text,
            ContentKind::Link,
            ContentKind::Color,
            ContentKind::Code,
            ContentKind::RichText,
            ContentKind::Image,
            ContentKind::Files,
            ContentKind::Video,
            ContentKind::Audio,
            ContentKind::Binary,
        ] {
            assert_eq!(ContentKind::from_token(k.as_str()), Some(k));
        }
        assert_eq!(ContentKind::from_token("nonsense"), None);
    }

    #[test]
    fn large_kinds_prefer_reference() {
        assert!(ContentKind::Video.prefers_reference());
        assert!(!ContentKind::Text.prefers_reference());
    }

    #[test]
    fn identical_content_dedups_to_same_id() {
        let a = NewClip {
            kind: ContentKind::Text,
            preview: "hello".into(),
            body: Some(b"hello".to_vec()),
            blob_ref: None,
            html: None,
            size: 5,
            id: None,
        };
        let b = a.clone();
        assert_eq!(a.dedup_id(), b.dedup_id());
    }

    #[test]
    fn different_kinds_do_not_collide() {
        let text = NewClip {
            kind: ContentKind::Text,
            preview: "x".into(),
            body: Some(b"x".to_vec()),
            blob_ref: None,
            html: None,
            size: 1,
            id: None,
        };
        let code = NewClip {
            kind: ContentKind::Code,
            ..text.clone()
        };
        assert_ne!(text.dedup_id(), code.dedup_id());
    }

    #[test]
    fn explicit_id_wins() {
        let c = NewClip {
            kind: ContentKind::Image,
            preview: "Image".into(),
            body: Some(b"thumbnail-bytes".to_vec()),
            blob_ref: Some("fullhash".into()),
            html: None,
            size: 999,
            id: Some("explicit-full-image-hash".into()),
        };
        assert_eq!(c.dedup_id(), "explicit-full-image-hash");
    }
}
