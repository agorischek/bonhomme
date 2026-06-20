use base64::{Engine, engine::general_purpose::STANDARD};

/// Marker prefixing the base64 of a binary file's bytes inside a `RenderedFile.content` string.
///
/// `RenderedFile.content` is a `String`, so non-UTF-8 bytes cannot be stored directly. The tree
/// walker wraps such bytes in this self-describing envelope; the blob handler stores and renders it
/// verbatim (so it round-trips through import → graph → render unchanged); and the file writer (the
/// `render` command) detects the envelope and decodes it back to raw bytes on disk. The leading NUL
/// plus a fixed tag makes a false positive on real source text effectively impossible — and the
/// walker only ever emits the envelope for files that fail UTF-8 decoding, so a text file never
/// receives it.
///
/// Inline base64 keeps binary files round-tripping today at the cost of bloating the op-log JSONB;
/// storing them content-addressed (a hash in the operation, bytes in a CAS / attachments table) is
/// the follow-up that keeps the log lean, with [`MAX_INLINE_BINARY_BYTES`] as the stopgap cap.
const BINARY_SENTINEL: &str = "\u{0}bonhomme-base64\u{0}";

/// Cap on the raw size of a binary file stored inline as base64. Larger files are skipped by the
/// walker with a warning until content-addressed storage lands. Base64 inflates by ~4/3, so this
/// bounds a single inline blob operation to roughly 7 MiB of text.
pub const MAX_INLINE_BINARY_BYTES: usize = 5 * 1024 * 1024;

/// Wrap raw bytes as a base64 binary envelope for storage in a `RenderedFile` / blob body.
pub fn encode_binary(bytes: &[u8]) -> String {
    format!("{BINARY_SENTINEL}{}", STANDARD.encode(bytes))
}

/// Whether `content` is a binary envelope rather than ordinary text.
pub fn is_binary(content: &str) -> bool {
    content.starts_with(BINARY_SENTINEL)
}

/// Decode a binary envelope back to raw bytes, or `None` if `content` is ordinary text (or its
/// payload is not valid base64).
pub fn decode_binary(content: &str) -> Option<Vec<u8>> {
    let payload = content.strip_prefix(BINARY_SENTINEL)?;
    STANDARD.decode(payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_round_trips_through_the_envelope() {
        let bytes = [0xFFu8, 0x00, 0xFE, 0x42, 0x00, 0x99];
        let envelope = encode_binary(&bytes);
        assert!(is_binary(&envelope));
        assert_eq!(decode_binary(&envelope).as_deref(), Some(&bytes[..]));
    }

    #[test]
    fn plain_text_is_not_mistaken_for_binary() {
        let text = "# A normal file\n\nwith text\n";
        assert!(!is_binary(text));
        assert_eq!(decode_binary(text), None);
    }
}
