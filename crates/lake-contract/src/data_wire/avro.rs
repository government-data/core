//! The manifest list a snapshot of the contract points to.

/// An Avro object container with Iceberg's manifest list schema and no
/// block: a snapshot that lists no manifest. Written by hand, as the
/// format is small: the magic, the metadata map, the sync marker.
pub(super) fn empty_manifest_list() -> Vec<u8> {
    const SCHEMA: &str = r#"{"type":"record","name":"manifest_file","fields":[
{"name":"manifest_path","type":"string","field-id":500},
{"name":"manifest_length","type":"long","field-id":501},
{"name":"partition_spec_id","type":"int","field-id":502},
{"name":"content","type":"int","field-id":517},
{"name":"sequence_number","type":"long","field-id":515},
{"name":"min_sequence_number","type":"long","field-id":516},
{"name":"added_snapshot_id","type":"long","field-id":503},
{"name":"added_files_count","type":"int","field-id":504},
{"name":"existing_files_count","type":"int","field-id":505},
{"name":"deleted_files_count","type":"int","field-id":506},
{"name":"added_rows_count","type":"long","field-id":512},
{"name":"existing_rows_count","type":"long","field-id":513},
{"name":"deleted_rows_count","type":"long","field-id":514}]}"#;
    fn long(out: &mut Vec<u8>, n: i64) {
        let mut z = ((n << 1) ^ (n >> 63)) as u64;
        loop {
            let byte = (z & 0x7f) as u8;
            z >>= 7;
            if z == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }
    fn bytes(out: &mut Vec<u8>, b: &[u8]) {
        long(out, i64::try_from(b.len()).expect("a length"));
        out.extend_from_slice(b);
    }
    let mut out = b"Obj\x01".to_vec();
    let meta: [(&str, &[u8]); 3] = [
        ("avro.schema", SCHEMA.as_bytes()),
        ("avro.codec", b"null"),
        ("format-version", b"2"),
    ];
    long(&mut out, 3);
    for (key, value) in meta {
        bytes(&mut out, key.as_bytes());
        bytes(&mut out, value);
    }
    long(&mut out, 0);
    out.extend_from_slice(b"contract-sync-16");
    out
}
