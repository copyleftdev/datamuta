use datamuta_kernel::Fingerprint;
use datamuta_plan::{Candidate, DataType, Field, FieldMapping, Operation, Schema};
use datamuta_postgres::{Endpoint, Verified};
use datamuta_stream::{
    Config, Plan, Result,
    spool::{Limits, Manifest},
};
fn config() -> Config {
    let schema = Schema {
        version: 1,
        namespace: "dm_test".into(),
        relation: "records".into(),
        fields: vec![Field {
            name: "id".into(),
            data_type: DataType::I64,
            nullable: false,
        }],
        primary_key: vec!["id".into()],
    };
    Config {
        version: 2,
        source_endpoint: Endpoint {
            port: 5432,
            namespace: "dm_test".into(),
            lab_only: true,
            verified: None,
        },
        source_engine: Default::default(),
        target_engine: Default::default(),
        target_endpoint: Endpoint {
            port: 5433,
            namespace: "dm_test".into(),
            lab_only: true,
            verified: None,
        },
        source: schema.clone(),
        target: schema,
        candidate: Candidate {
            version: 1,
            fields: vec![FieldMapping {
                source: "id".into(),
                target: "id".into(),
                operation: Operation::Copy,
            }],
        },
        limits: Limits::default(),
    }
}
fn plan(c: Config) -> Result<Plan> {
    let limits = c.limits.clone();
    Plan::new(
        c,
        Manifest {
            version: 1,
            source: Fingerprint::of(b"source capture"),
            root: Fingerprint::of(b"sealed root"),
            rows: 1,
            partitions: 1,
            payload_bytes: 10,
            limits,
        },
    )
}
#[test]
fn endpoint_namespaces_and_self_copy_are_checked_independently() -> Result<()> {
    let c = config();
    c.compile()?; // Same qualified relation on separate ports is valid.
    let mut bad = c.clone();
    bad.source_endpoint.namespace = "dm_other".into();
    assert!(bad.compile().is_err());
    let mut bad = c.clone();
    bad.target_endpoint.namespace = "dm_other".into();
    assert!(bad.compile().is_err());
    let mut bad = c.clone();
    bad.target_endpoint = c.source_endpoint.clone();
    assert!(bad.compile().is_err());
    // A different role/trust setting must not disguise the same configured database.
    bad.target_endpoint.lab_only = false;
    bad.target_endpoint.verified = Some(Verified {
        host: "127.0.0.1".into(),
        database: "datamuta".into(),
        user: "another".into(),
        ca_der: "/not/read/ca.der".into(),
        ca_sha256: Fingerprint::of(b"ca"),
        password_env: "DATAMUTA_TARGET_PASSWORD".into(),
    });
    assert!(bad.compile().is_err());
    let mut separate_schema = c.clone();
    separate_schema.target_endpoint.port = 5432;
    separate_schema.target_endpoint.namespace = "dm_target".into();
    separate_schema.target.namespace = "dm_target".into();
    separate_schema.compile()?;
    let mut old = c;
    old.version = 1;
    assert!(old.compile().is_err());
    let mut json = serde_json::to_value(config())?;
    json.as_object_mut()
        .ok_or_else(|| std::io::Error::other("object"))?
        .remove("source_endpoint");
    json["endpoint"] = serde_json::to_value(config().source_endpoint)?;
    assert!(serde_json::from_value::<Config>(json).is_err());
    Ok(())
}
#[test]
fn both_origins_and_destination_are_bound_to_approval() -> Result<()> {
    let p = plan(config())?;
    let approved = p.fingerprint()?;
    let mut source = p.clone();
    source.config.source_endpoint.port = 5540;
    assert_ne!(source.fingerprint()?, approved);
    assert!(source.approve(&approved).is_err());
    let mut target = p.clone();
    target.config.target_endpoint.port = 5541;
    assert_ne!(target.fingerprint()?, approved);
    assert!(target.approve(&approved).is_err());
    let mut version = p;
    version.version = 1;
    assert!(version.validate().is_err());
    Ok(())
}
#[test]
fn offline_plan_validation_does_not_load_source_credentials() -> Result<()> {
    let mut c = config();
    c.source_endpoint.lab_only = false;
    c.source_endpoint.verified = Some(Verified {
        host: "source.example".into(),
        database: "source".into(),
        user: "reader".into(),
        ca_der: "/missing/source-ca.der".into(),
        ca_sha256: Fingerprint::of(b"source ca"),
        password_env: "DATAMUTA_ABSENT_SOURCE_PASSWORD".into(),
    });
    let mut p = plan(c)?;
    let approved = p.fingerprint()?;
    let v = p
        .config
        .source_endpoint
        .verified
        .as_mut()
        .ok_or_else(|| std::io::Error::other("verified source"))?;
    v.password_env = "DATAMUTA_ROTATED_SOURCE_PASSWORD".into();
    v.ca_der = "/also/missing.der".into();
    p.approve(&approved)?;
    p.config
        .source_endpoint
        .verified
        .as_mut()
        .ok_or_else(|| std::io::Error::other("verified source"))?
        .user = "different_reader".into();
    assert!(p.approve(&approved).is_err());
    Ok(())
}

#[test]
fn target_engine_is_approval_bound_and_new_engines_are_lab_only() -> Result<()> {
    use datamuta_stream::TargetEngine;
    let original = plan(config())?;
    for engine in [
        TargetEngine::Mysql,
        TargetEngine::Mongo,
        TargetEngine::Firestore,
    ] {
        let mut changed = original.clone();
        changed.config.target_engine = engine;
        assert_ne!(changed.fingerprint()?, original.fingerprint()?);
        assert!(changed.approve(&original.fingerprint()?).is_err());
        changed.config.target_endpoint.lab_only = false;
        assert!(changed.validate().is_err());
    }
    let mut malformed = serde_json::to_value(config())?;
    malformed["target_engine"] = serde_json::json!("any_nosql");
    assert!(serde_json::from_value::<Config>(malformed).is_err());
    Ok(())
}
#[test]
fn mongo_contract_reserves_native_identity_and_is_order_independent() -> Result<()> {
    let mut schema = config().target;
    schema.fields.push(Field {
        name: "label".into(),
        data_type: DataType::Utf8,
        nullable: true,
    });
    let validator = datamuta_stream::mongo::validator(&schema)?;
    schema.fields.reverse();
    assert_eq!(validator, datamuta_stream::mongo::validator(&schema)?);
    schema.fields.push(Field {
        name: "_id".into(),
        data_type: DataType::I64,
        nullable: false,
    });
    assert!(datamuta_stream::mongo::validator(&schema).is_err());
    Ok(())
}

#[test]
fn source_engine_is_bound_and_same_firestore_collection_refused() -> Result<()> {
    use datamuta_stream::{SourceEngine, TargetEngine};
    let original = plan(config())?;
    let mut changed = original.clone();
    changed.config.source_engine = SourceEngine::Firestore;
    assert_ne!(original.fingerprint()?, changed.fingerprint()?);
    assert!(changed.approve(&original.fingerprint()?).is_err());
    changed.config.target_engine = TargetEngine::Firestore;
    changed.config.target_endpoint = changed.config.source_endpoint.clone();
    assert!(changed.validate().is_err());
    changed.config.target.relation = "other".into();
    assert!(changed.validate().is_ok());
    changed.config.source_endpoint.lab_only = false;
    assert!(changed.validate().is_err());
    Ok(())
}
