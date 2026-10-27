use super::*;
use datamuta_kernel::{MigrationUnit, canonical_hash};

fn fixture() -> (Schema, Schema, Candidate) {
    let source = Schema {
        version: 1,
        namespace: "src".into(),
        relation: "orders".into(),
        fields: vec![
            Field {
                name: "id".into(),
                data_type: DataType::I32,
                nullable: false,
            },
            Field {
                name: "memo".into(),
                data_type: DataType::Utf8,
                nullable: true,
            },
            Field {
                name: "active".into(),
                data_type: DataType::Bool,
                nullable: false,
            },
        ],
        primary_key: vec!["id".into()],
    };
    let mut target = source.clone();
    target.namespace = "dst".into();
    target.fields[0].name = "key".into();
    target.fields[0].data_type = DataType::I64;
    target.primary_key = vec!["key".into()];
    let candidate = Candidate {
        version: 1,
        fields: vec![
            FieldMapping {
                source: "id".into(),
                target: "key".into(),
                operation: Operation::WidenI32ToI64,
            },
            FieldMapping {
                source: "memo".into(),
                target: "memo".into(),
                operation: Operation::Copy,
            },
            FieldMapping {
                source: "active".into(),
                target: "active".into(),
                operation: Operation::Copy,
            },
        ],
    };
    (source, target, candidate)
}
fn row(id: i32, memo: Scalar) -> Record {
    [
        ("id".into(), Scalar::I32(id)),
        ("memo".into(), memo),
        ("active".into(), Scalar::Bool(true)),
    ]
    .into()
}
#[test]
fn lossless_values_include_boundaries_null_empty_and_unicode() -> Result<()> {
    let (s, t, c) = fixture();
    let m = compile(&s, &t, &c)?;
    let rows = vec![
        row(i32::MIN, Scalar::Null),
        row(0, Scalar::Utf8("".into())),
        row(i32::MAX, Scalar::Utf8("雪🦀\n'\\".into())),
    ];
    let units = m.prepare(&rows)?;
    for (n, u) in units.iter().enumerate() {
        assert_eq!(u.ordinal()?, n as u64);
        assert_eq!(u.fields()["memo"], rows[n]["memo"]);
        assert_eq!(u.fields()["active"], Scalar::Bool(true));
    }
    assert_eq!(units[0].fields()["key"], Scalar::I64(i64::from(i32::MIN)));
    assert_eq!(units[2].fields()["key"], Scalar::I64(i64::from(i32::MAX)));
    Ok(())
}
#[test]
fn normalization_is_deterministic_and_semantic_drift_changes_contract() -> Result<()> {
    let (mut s, mut t, mut c) = fixture();
    let a = compile(&s, &t, &c)?;
    s.fields.reverse();
    t.fields.reverse();
    c.fields.reverse();
    let b = compile(&s, &t, &c)?;
    assert_eq!(a.contract()?, b.contract()?);
    assert_eq!(
        canonical_hash(&a.prepare(&[row(1, Scalar::Null)])?)?,
        canonical_hash(&b.prepare(&[row(1, Scalar::Null)])?)?
    );
    t.relation = "different".into();
    let changed = compile(&s, &t, &c)?;
    assert_ne!(a.contract()?, changed.contract()?);
    assert_eq!(a.validate_observed(&s, &t), Err(Error::Drift));
    Ok(())
}
#[test]
fn compiler_refuses_loss_ambiguity_missing_coverage_and_identity_changes() {
    let (s, t, c) = fixture();
    let mut bad = c.clone();
    bad.fields.pop();
    assert!(compile(&s, &t, &bad).is_err());
    let mut bad = c.clone();
    bad.fields[1].target = "key".into();
    assert!(compile(&s, &t, &bad).is_err());
    let mut bad = c.clone();
    bad.fields[1].source = "absent".into();
    assert!(compile(&s, &t, &bad).is_err());
    let mut bad = c.clone();
    bad.fields[0].operation = Operation::Copy;
    assert!(compile(&s, &t, &bad).is_err());
    let mut bad = t.clone();
    bad.fields[1].nullable = false;
    assert!(compile(&s, &bad, &c).is_err());
    let mut bad = t.clone();
    bad.fields[2].data_type = DataType::I64;
    bad.primary_key = vec!["active".into()];
    assert!(compile(&s, &bad, &c).is_err());
    assert!(
        compile(
            &t,
            &s,
            &Candidate {
                version: 1,
                fields: c
                    .fields
                    .iter()
                    .map(|f| FieldMapping {
                        source: f.target.clone(),
                        target: f.source.clone(),
                        operation: f.operation
                    })
                    .collect()
            }
        )
        .is_err()
    );
}
#[test]
fn schema_and_wire_formats_fail_closed() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let (s, t, c) = fixture();
    let mut bad = s.clone();
    bad.fields.push(bad.fields[0].clone());
    assert!(bad.validate().is_err());
    let mut bad = s.clone();
    bad.primary_key.clear();
    assert!(bad.validate().is_err());
    let mut bad = s.clone();
    bad.namespace = "public;DROP".into();
    assert!(bad.validate().is_err());
    let mut bad = s.clone();
    bad.fields[0].nullable = true;
    assert!(bad.validate().is_err());
    let mut bad = c.clone();
    bad.version = 2;
    assert_eq!(compile(&s, &t, &bad).err(), Some(Error::Version(2)));
    let mut value = serde_json::to_value(&c)?;
    value["sql"] = serde_json::json!("DELETE FROM target");
    assert!(serde_json::from_value::<Candidate>(value).is_err());
    let mut value = serde_json::to_value(&s)?;
    value["fields"][0]["data_type"] = serde_json::json!("float64");
    assert!(serde_json::from_value::<Schema>(value).is_err());
    Ok(())
}
#[test]
fn runtime_rejects_type_drift_missing_fields_duplicates_and_excess_bytes() -> Result<()> {
    let (s, t, c) = fixture();
    let m = compile(&s, &t, &c)?;
    let valid = row(1, Scalar::Null);
    assert!(m.prepare(&[valid.clone(), valid.clone()]).is_err());
    let mut bad = valid.clone();
    bad.remove("memo");
    assert!(m.prepare(&[bad]).is_err());
    let mut bad = valid.clone();
    bad.insert("id".into(), Scalar::I64(1));
    assert!(m.prepare(&[bad]).is_err());
    let mut bad = valid.clone();
    bad.insert("active".into(), Scalar::Null);
    assert!(m.prepare(&[bad]).is_err());
    assert!(
        m.prepare(&[row(1, Scalar::Utf8("x".repeat(32_769)))])
            .is_err()
    );
    // Escaping can exceed the encoded bound even within the raw text bound.
    assert!(
        m.prepare(&[row(1, Scalar::Utf8("\0".repeat(12_000)))])
            .is_err()
    );
    assert!(m.prepare(&[]).is_err());
    assert!(m.prepare(&vec![valid; 4097]).is_err());
    Ok(())
}
#[test]
fn composite_identity_order_and_uniqueness_are_preserved() -> Result<()> {
    let (mut s, mut t, mut c) = fixture();
    for schema in [&mut s, &mut t] {
        schema.fields.push(Field {
            name: "tenant".into(),
            data_type: DataType::I32,
            nullable: false,
        });
        schema.primary_key.insert(0, "tenant".into());
    }
    c.fields.push(FieldMapping {
        source: "tenant".into(),
        target: "tenant".into(),
        operation: Operation::Copy,
    });
    let m = compile(&s, &t, &c)?;
    let mut a = row(1, Scalar::Null);
    a.insert("tenant".into(), Scalar::I32(1));
    let mut b = a.clone();
    b.insert("tenant".into(), Scalar::I32(2));
    assert_eq!(m.prepare(&[a, b])?.len(), 2);
    t.primary_key.reverse();
    assert!(compile(&s, &t, &c).is_err());
    Ok(())
}
