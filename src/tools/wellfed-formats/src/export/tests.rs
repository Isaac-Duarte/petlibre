use uuid::Uuid;

use super::*;

fn user(n: u128, name: &str) -> Row {
    Row::Users(User {
        id: Uuid::from_u128(n),
        name: name.into(),
        role: "owner".into(),
        created_ms: 1_790_000_000_000,
        expires_ms: None,
    })
}

fn event() -> Row {
    Row::Events(Event {
        feeder_id: Uuid::from_u128(9),
        epoch: 3,
        seq: 1,
        at_s: None,
        received_ms: 5,
        kind: "fed".into(),
        body: vec![0x00, 0xab, 0xff],
    })
}

/// An archive of `rows` in `encoding`, written the way the server writes
/// one: the manifest first, then each table's file.
fn archive(rows: &[Row], encoding: Encoding) -> Vec<u8> {
    let mut files = Vec::new();
    let mut tables = Vec::new();
    for &table in Table::ALL {
        let mut file = file_header(table, encoding);
        let mut sum = TableSum::new(table);
        sum.add_bytes(&file);
        for row in rows.iter().filter(|row| row.table() == table) {
            let mut line = Vec::new();
            row.write(encoding, &mut line).unwrap();
            sum.add_row(&line);
            file.extend_from_slice(&line);
        }
        tables.push(sum.finish());
        files.push((file_name(table, encoding), file));
    }
    let manifest = Manifest {
        format: FORMAT_VERSION,
        encoding,
        server: "0.1.0".into(),
        tables,
    };

    let mut out = Vec::new();
    let mut entry = |name: &str, data: &[u8]| {
        out.extend_from_slice(&header(name, data.len() as u64).unwrap());
        out.extend_from_slice(data);
        out.extend_from_slice(padding(data.len() as u64));
    };
    entry(MANIFEST, &manifest.to_bytes().unwrap());
    for (name, data) in &files {
        entry(name, data);
    }
    out.extend_from_slice(&END);
    out
}

/// Everything `bytes` yields, pushed `chunk` bytes at a time.
fn read(bytes: &[u8], chunk: usize) -> Result<Vec<Item>, ArchiveError> {
    let mut reader = Reader::new();
    let mut items = Vec::new();
    for piece in bytes.chunks(chunk) {
        reader.push(piece)?;
        while let Some(item) = reader.next_item()? {
            items.push(item);
        }
    }
    reader.finish()?;
    Ok(items)
}

fn rows(items: &[Item]) -> Vec<Row> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Row(row) => Some(Row::clone(row)),
            _ => None,
        })
        .collect()
}

#[test]
fn an_archive_reads_back_its_rows_in_any_chunks() {
    let written = vec![user(1, "ada"), user(2, "bob"), event()];
    let bytes = archive(&written, Encoding::Json);

    for chunk in [1, 7, 512, bytes.len()] {
        let items = read(&bytes, chunk).unwrap();

        assert_eq!(rows(&items), written, "pushed {chunk} at a time");
        assert!(matches!(items.first(), Some(Item::Manifest(_))));
        assert!(matches!(items.last(), Some(Item::End)));
        let ended = items
            .iter()
            .filter(|item| matches!(item, Item::Table(_)))
            .count();
        assert_eq!(ended, Table::ALL.len());
    }
}

#[test]
fn a_row_is_one_line_of_canonical_json_with_hex_bytes() {
    let mut line = Vec::new();
    event().write(Encoding::Json, &mut line).unwrap();

    assert_eq!(
        String::from_utf8(line).unwrap(),
        "{\"at_s\":null,\"body\":\"00abff\",\"epoch\":3,\"feeder_id\":\"00000000-0000-0000-0000-000000000009\",\"kind\":\"fed\",\"received_ms\":5,\"seq\":1}\n"
    );
}

#[test]
fn a_csv_file_has_the_columns_in_order_then_a_record_per_row() {
    let mut out = file_header(Table::Events, Encoding::Csv);
    event().write(Encoding::Csv, &mut out).unwrap();

    assert_eq!(
        String::from_utf8(out).unwrap(),
        "feeder_id,epoch,seq,at_s,received_ms,kind,body\r\n00000000-0000-0000-0000-000000000009,3,1,,5,fed,00abff\r\n"
    );
}

#[test]
fn every_table_has_columns() {
    for &table in Table::ALL {
        assert!(!table.columns().is_empty(), "{table:?}");
    }
    assert!(!Table::Users.columns().contains(&"password_hash"));
    assert!(!Table::Feeders.columns().contains(&"key_sealed"));
}

#[test]
fn a_csv_archive_does_not_import() {
    let bytes = archive(&[user(1, "ada")], Encoding::Csv);

    assert!(matches!(read(&bytes, 512), Err(ArchiveError::Csv)));
}

#[test]
fn a_changed_row_fails_its_tables_hash() {
    let mut bytes = archive(&[user(1, "ada")], Encoding::Json);
    let at = bytes
        .windows(3)
        .position(|window| window == b"ada")
        .unwrap();
    bytes[at] = b'e';

    assert!(matches!(
        read(&bytes, 512),
        Err(ArchiveError::Hash(Table::Users))
    ));
}

#[test]
fn a_cut_archive_is_truncated() {
    let bytes = archive(&[user(1, "ada")], Encoding::Json);

    let cut = &bytes[..bytes.len() - END.len() - BLOCK];
    assert!(matches!(read(cut, 512), Err(ArchiveError::Truncated)));

    // One empty block of the two is not the end.
    let cut = &bytes[..bytes.len() - BLOCK];
    assert!(matches!(read(cut, 512), Err(ArchiveError::Truncated)));
}

#[test]
fn bytes_past_the_declared_length_are_refused() {
    let mut bytes = archive(&[user(1, "ada")], Encoding::Json);
    bytes.extend_from_slice(&[0; BLOCK]);

    for chunk in [512, bytes.len()] {
        assert!(
            matches!(read(&bytes, chunk), Err(ArchiveError::TooLong)),
            "{chunk}"
        );
    }
}

#[test]
fn another_format_version_is_refused_before_its_tables() {
    let next = FORMAT_VERSION + 1;
    let manifest = format!("{{\"format\":{next},\"something\":\"new\"}}\n");
    let manifest = manifest.as_bytes();
    let mut bytes = header(MANIFEST, manifest.len() as u64).unwrap().to_vec();
    bytes.extend_from_slice(manifest);
    bytes.extend_from_slice(padding(manifest.len() as u64));

    assert!(matches!(
        read(&bytes, 512),
        Err(ArchiveError::Version { found }) if found == next
    ));
}

#[test]
fn a_line_past_the_limit_is_refused_without_holding_the_file() {
    let line = vec![b'x'; MAX_LINE + 2];
    let mut sum = TableSum::new(Table::Users);
    sum.add_row(&line);
    let mut tables: Vec<TableFile> = Table::ALL
        .iter()
        .map(|&t| TableSum::new(t).finish())
        .collect();
    tables[0] = sum.finish();
    let manifest = Manifest {
        format: FORMAT_VERSION,
        encoding: Encoding::Json,
        server: "0.1.0".into(),
        tables,
    }
    .to_bytes()
    .unwrap();
    let mut bytes = header(MANIFEST, manifest.len() as u64).unwrap().to_vec();
    bytes.extend_from_slice(&manifest);
    bytes.extend_from_slice(padding(manifest.len() as u64));
    bytes.extend_from_slice(&header("users.jsonl", line.len() as u64).unwrap());
    bytes.extend_from_slice(&line);

    assert!(matches!(
        read(&bytes, 4096),
        Err(ArchiveError::LineTooLong(Table::Users))
    ));
}
