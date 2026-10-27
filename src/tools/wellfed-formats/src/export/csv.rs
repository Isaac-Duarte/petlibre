//! A table as RFC 4180 CSV: a header row of its columns, then a record per
//! row with the cells in the same order. A null is an empty cell; a cell
//! holding a comma, a quote or a line break is quoted, its quotes doubled.

use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};
use serde_json::Value;

/// The header row.
pub(super) fn header(columns: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        cell(column, &mut out);
    }
    out.extend_from_slice(b"\r\n");
    out
}

/// One record: `row`'s fields, named by `columns`. A row is an object of
/// scalars, so nothing is nested.
pub(super) fn record(columns: &[&str], row: &Value, out: &mut Vec<u8>) {
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        match row.get(column) {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) => cell(text, out),
            Some(Value::Bool(flag)) => {
                out.extend_from_slice(if *flag { b"true" } else { b"false" })
            }
            Some(other) => cell(&other.to_string(), out),
        }
    }
    out.extend_from_slice(b"\r\n");
}

fn cell(text: &str, out: &mut Vec<u8>) {
    if !text.contains([',', '"', '\n', '\r']) {
        out.extend_from_slice(text.as_bytes());
        return;
    }
    out.push(b'"');
    out.extend_from_slice(text.replace('"', "\"\"").as_bytes());
    out.push(b'"');
}

/// A struct's field names, in declaration order, as its derived
/// `Deserialize` names them: the columns of a table with no row to look at.
pub(super) fn field_names<'de, T: Deserialize<'de>>() -> &'static [&'static str] {
    let mut names: &'static [&'static str] = &[];
    // The deserializer only records the names and refuses, so the error
    // is expected and carries nothing.
    let _refused = T::deserialize(Names(&mut names));
    names
}

/// A deserializer that answers only the question of a struct's fields.
struct Names<'a>(&'a mut &'static [&'static str]);

#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("only a struct's field names are read")
    }
}

impl std::error::Error for Refused {}

impl de::Error for Refused {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self
    }
}

impl<'de> Deserializer<'de> for Names<'_> {
    type Error = Refused;

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Refused> {
        *self.0 = fields;
        Err(Refused)
    }

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Refused> {
        Err(Refused)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Deserialize)]
    #[expect(dead_code, reason = "only the field names are read")]
    struct Pair {
        first: u8,
        second: Option<String>,
    }

    #[test]
    fn a_structs_fields_are_its_columns_in_order() {
        assert_eq!(field_names::<Pair>(), ["first", "second"]);
    }

    #[test]
    fn cells_are_quoted_only_when_they_must_be() {
        let columns = ["a", "b", "c", "d", "e"];
        let row = json!({"a": 7, "b": "plain", "c": "say \"hi\", twice", "d": null, "e": true});

        let mut out = header(&columns);
        record(&columns, &row, &mut out);

        assert_eq!(
            String::from_utf8(out).unwrap(),
            "a,b,c,d,e\r\n7,plain,\"say \"\"hi\"\", twice\",,true\r\n"
        );
    }
}
