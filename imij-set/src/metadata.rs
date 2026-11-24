pub use imij::item_metadata::{ItemMetadata as EntryMetadata, Value};
use std::io;
pub(crate) fn encode(metadata: &EntryMetadata) -> io::Result<Vec<u8>> {
    metadata
        .to_bytes()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
pub(crate) fn decode(bytes: &[u8]) -> io::Result<EntryMetadata> {
    EntryMetadata::from_bytes(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_item_bytes_preserve_existing_index_layout() {
        let mut value = EntryMetadata::default();
        value.filename = Some("x".into());
        value.attributes.insert("q".into(), Value::Integer(-7));
        let mut expected = vec![1, 1, b'x', 1, 1, b'q', 3];
        expected.extend_from_slice(&(-7i64).to_le_bytes());
        assert_eq!(encode(&value).unwrap(), expected);
        assert_eq!(decode(&expected).unwrap(), value);
        let mut metadata = imij::Metadata::default();
        value.write_metadata(&mut metadata).unwrap();
        assert_eq!(EntryMetadata::from_metadata(&metadata).unwrap(), value);
    }
}
