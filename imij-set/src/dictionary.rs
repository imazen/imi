//! Per-leaf string coding; shared item metadata remains the semantic authority.
use crate::wire::{Cursor, MAX_PAGE, bad, put};
use std::collections::BTreeMap;
use std::io;

const MAX_METADATA: usize = 16 * 1024;

fn append(out: &mut Vec<u8>, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_METADATA.saturating_sub(out.len()) {
        return Err(bad("expanded index metadata size"));
    }
    out.extend_from_slice(bytes);
    Ok(())
}
fn text<'a>(c: &mut Cursor<'a>) -> io::Result<&'a [u8]> {
    let len = usize::try_from(c.var()?).map_err(|_| bad("dictionary string length"))?;
    let bytes = c.take(len)?;
    std::str::from_utf8(bytes).map_err(|_| bad("dictionary UTF-8"))?;
    Ok(bytes)
}
fn literal(out: &mut Vec<u8>, bytes: &[u8]) -> io::Result<()> {
    let mut prefix = Vec::new();
    put(&mut prefix, bytes.len() as u64);
    append(out, &prefix)?;
    append(out, bytes)
}

// Rewrite only the string representation. The ordinary metadata parser still
// validates names, sorted/unique keys, value types, finite floats and booleans.
fn rewrite<'a>(
    data: &'a [u8],
    mut string: impl FnMut(&mut Cursor<'a>, &mut Vec<u8>) -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if data.len() > MAX_METADATA {
        return Err(bad("index metadata size"));
    }
    let mut c = Cursor(data);
    let mut out = Vec::new();
    let filename = c.byte()?;
    append(&mut out, &[filename])?;
    match filename {
        0 => {}
        1 => string(&mut c, &mut out)?,
        _ => return Err(bad("index filename flag")),
    }
    let count = c.var()?;
    if count > 64 {
        return Err(bad("too many index attributes"));
    }
    put(&mut out, count);
    for _ in 0..count {
        string(&mut c, &mut out)?;
        let tag = c.byte()?;
        append(&mut out, &[tag])?;
        match tag {
            0 => string(&mut c, &mut out)?,
            1 => {
                let n = usize::try_from(c.var()?).map_err(|_| bad("index bytes length"))?;
                literal(&mut out, c.take(n)?)?;
            }
            2..=4 => append(&mut out, c.take(8)?)?,
            5 => append(&mut out, c.take(1)?)?,
            _ => return Err(bad("index metadata type")),
        }
    }
    c.end()?;
    if out.len() > MAX_METADATA {
        return Err(bad("index metadata size"));
    }
    Ok(out)
}

/// Compress the ordinary length-prefixed metadata column. Absence means the
/// candidate is inapplicable; the caller compares complete page sizes.
pub(crate) fn compress(column: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut strings = BTreeMap::<&[u8], usize>::new();
    let mut input = Cursor(column);
    while !input.0.is_empty() {
        let n = usize::try_from(input.var().ok()?).ok()?;
        rewrite(input.take(n).ok()?, |c, out| {
            let value = text(c)?;
            *strings.entry(value).or_default() += 1;
            literal(out, value)
        })
        .ok()?;
    }
    // Singletons remain literals; very short strings cannot amortize a table.
    let dictionary: Vec<_> = strings
        .into_iter()
        .filter_map(|(s, count)| (count > 1 && s.len() > 1).then_some(s))
        .collect();
    if dictionary.is_empty() {
        return None;
    }
    let mut table = Vec::new();
    put(&mut table, dictionary.len() as u64);
    for value in &dictionary {
        put(&mut table, value.len() as u64);
        table.extend_from_slice(value);
    }
    if table.len() > MAX_PAGE {
        return None;
    }
    let mut output = Vec::new();
    let mut input = Cursor(column);
    while !input.0.is_empty() {
        let n = usize::try_from(input.var().ok()?).ok()?;
        let encoded = rewrite(input.take(n).ok()?, |c, out| {
            let value = text(c)?;
            match dictionary.binary_search(&value) {
                Ok(index) => put(out, ((index as u64) << 1) | 1),
                Err(_) => {
                    put(out, (value.len() as u64) << 1);
                    append(out, value)?;
                }
            }
            Ok(())
        })
        .ok()?;
        put(&mut output, encoded.len() as u64);
        output.extend_from_slice(&encoded);
    }
    Some((output, table))
}

/// Borrow dictionary bytes from the already bounded and checksummed page.
pub(crate) fn table(bytes: &[u8]) -> io::Result<Vec<&[u8]>> {
    let mut c = Cursor(bytes);
    let n = usize::try_from(c.var()?).map_err(|_| bad("dictionary count"))?;
    // Each nonempty string consumes a length byte and at least one data byte.
    if n == 0 || n > c.0.len() / 2 || bytes.len() > MAX_PAGE {
        return Err(bad("dictionary count"));
    }
    let mut table = Vec::with_capacity(n);
    for _ in 0..n {
        let value = text(&mut c)?;
        if value.is_empty()
            || value.len() > MAX_METADATA
            || table.last().is_some_and(|last| last >= &value)
        {
            return Err(bad("dictionary order/string size"));
        }
        table.push(value);
    }
    c.end()?;
    Ok(table)
}

pub(crate) fn expand(bytes: &[u8], dictionary: &[&[u8]]) -> io::Result<Vec<u8>> {
    rewrite(bytes, |c, out| {
        let token = c.var()?;
        let value = if token & 1 == 1 {
            let index = usize::try_from(token >> 1).map_err(|_| bad("dictionary reference"))?;
            *dictionary
                .get(index)
                .ok_or_else(|| bad("dictionary reference"))?
        } else {
            let n = usize::try_from(token >> 1).map_err(|_| bad("dictionary literal length"))?;
            c.take(n)?
        };
        literal(out, value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryMetadata, Value};

    #[test]
    fn all_metadata_types_share_strings_without_changing_item_bytes() {
        let mut meta = EntryMetadata::default();
        meta.filename = Some("variants/猫.imij".into());
        for (key, value) in [
            ("codec", Value::Text("zenjpeg".into())),
            ("alpha", Value::Bool(true)),
            ("bytes", Value::Bytes(vec![0, 255, 0])),
            ("unsigned", Value::Unsigned(u64::MAX)),
            ("signed", Value::Integer(i64::MIN)),
            ("float", Value::Float(-0.0)),
            ("empty", Value::Text(String::new())),
            ("null-in-text", Value::Text("x\0y".into())),
        ] {
            meta.attributes.insert(key.into(), value);
        }
        let original = crate::metadata::encode(&meta).unwrap();
        let mut column = Vec::new();
        for _ in 0..100 {
            put(&mut column, original.len() as u64);
            column.extend_from_slice(&original);
        }
        let (encoded, table_bytes) = compress(&column).unwrap();
        assert!(encoded.len() + table_bytes.len() < column.len());
        let table = table(&table_bytes).unwrap();
        let mut cursor = Cursor(&encoded);
        for _ in 0..100 {
            let len = cursor.var().unwrap() as usize;
            let expanded = expand(cursor.take(len).unwrap(), &table).unwrap();
            assert_eq!(expanded, original);
            assert_eq!(crate::metadata::decode(&expanded).unwrap(), meta);
        }
        cursor.end().unwrap();
    }

    #[test]
    fn bad_tables_references_and_expansions_are_errors() {
        for bytes in [
            vec![0],
            vec![255, 127],
            vec![1, 1, 255],
            vec![2, 1, b'a', 1, b'a'],
            vec![2, 1, b'z', 1, b'a'],
        ] {
            assert!(table(&bytes).is_err());
        }
        let table = [b"name".as_slice()];
        assert!(expand(&[1, 3, 0], &table).is_err()); // filename reference #1
        assert!(expand(&[1, 128], &table).is_err()); // truncated token
        assert!(expand(&[2, 0], &table).is_err()); // closed filename flag
        assert!(expand(&[0, 65], &table).is_err()); // too many attributes
        let huge = vec![b'x'; MAX_METADATA];
        // One reference cannot evade the ordinary per-entry metadata bound.
        assert!(expand(&[1, 1, 0], &[&huge]).is_err());
        // String coding does not bypass the shared semantic validation.
        let duplicate = expand(&[0, 2, 1, 5, 1, 1, 5, 0], &table).unwrap();
        assert!(crate::metadata::decode(&duplicate).is_err());
        let invalid_bool = expand(&[0, 1, 1, 5, 2], &table).unwrap();
        assert!(crate::metadata::decode(&invalid_bool).is_err());
    }
}
