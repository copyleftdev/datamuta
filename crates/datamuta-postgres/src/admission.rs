//! Native metadata and privilege checks; no mutations or implicit provisioning.
use super::*;

pub(super) async fn metadata(c: &Client, namespace: &str) -> Result<()> {
    for (table, id_type, value, value_type) in [
        ("receipts", DataType::I64, "hash", DataType::Utf8),
        ("progress", DataType::I32, "frontier", DataType::I64),
    ] {
        let observed = describe(c, namespace, table).await?;
        let expected = Schema {
            version: 1,
            namespace: namespace.into(),
            relation: table.into(),
            fields: vec![
                Field {
                    name: "id".into(),
                    data_type: id_type,
                    nullable: false,
                },
                Field {
                    name: value.into(),
                    data_type: value_type,
                    nullable: false,
                },
            ],
            primary_key: vec!["id".into()],
        };
        require(
            observed == expected,
            "unsupported receipt or checkpoint schema",
        )?;
    }
    // LIMIT avoids an unbounded scan of accidentally misprovisioned metadata.
    let rows = c
        .query(
            &format!("SELECT id,frontier FROM {}.progress LIMIT 2", q(namespace)),
            &[],
        )
        .await
        .map_err(err)?;
    require(
        rows.len() == 1
            && rows[0].try_get::<_, i32>(0).map_err(err)? == 1
            && rows[0].try_get::<_, i64>(1).map_err(err)? >= 0,
        "checkpoint must contain exactly one nonnegative row with id=1",
    )
}

pub(super) async fn mapping(c: &Client, m: &CompiledMapping) -> Result<()> {
    let source = describe(c, &m.source().namespace, &m.source().relation).await?;
    let target = describe(c, &m.target().namespace, &m.target().relation).await?;
    m.validate_observed(&source, &target).map_err(err)?;
    metadata(c, &target.namespace).await?;
    let namespace = &target.namespace;
    let allowed: bool = c
        .query_one("SELECT has_schema_privilege($1, 'USAGE')", &[namespace])
        .await
        .map_err(err)?
        .try_get(0)
        .map_err(err)?;
    require(allowed, "schema USAGE required")?;
    // SHARE source locking requires UPDATE, DELETE or TRUNCATE privileges. Admit
    // UPDATE only; no source writes are performed. ROW EXCLUSIVE on destination
    // and metadata also needs UPDATE, and blocks trigger/FK DDL that ACCESS SHARE
    // would permit to race with catalog validation.
    for (table, permissions) in [
        (source.relation.as_str(), &["SELECT", "UPDATE"][..]),
        (
            target.relation.as_str(),
            &["SELECT", "INSERT", "UPDATE"][..],
        ),
        ("receipts", &["SELECT", "INSERT", "UPDATE"][..]),
        ("progress", &["SELECT", "UPDATE"][..]),
    ] {
        let name = format!("{}.{}", q(namespace), q(table));
        for permission in permissions {
            let yes: bool = c
                .query_one("SELECT has_table_privilege($1,$2)", &[&name, permission])
                .await
                .map_err(err)?
                .try_get(0)
                .map_err(err)?;
            require(yes, "required table privilege missing")?;
        }
    }
    Ok(())
}
