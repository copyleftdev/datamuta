use datamuta_plan::{DataType, Evidence, Field, MODEL, Schema};
use datamuta_stream::{
    Result,
    suggest::{self, Admitted, Prepared},
};
use std::collections::BTreeMap;

fn field(name: &str, data_type: DataType) -> Field {
    Field {
        name: name.into(),
        data_type,
        nullable: false,
    }
}
/// Two bigint amounts make two structurally valid mappings.
fn ambiguous() -> (Schema, Schema) {
    let source = Schema {
        version: 1,
        namespace: "src".into(),
        relation: "orders".into(),
        fields: vec![
            field("order_id", DataType::I32),
            field("cents", DataType::I64),
            field("tax_cents", DataType::I64),
        ],
        primary_key: vec!["order_id".into()],
    };
    let target = Schema {
        version: 1,
        namespace: "dst".into(),
        relation: "purchases".into(),
        fields: vec![
            field("order_key", DataType::I64),
            field("tax", DataType::I64),
            field("total", DataType::I64),
        ],
        primary_key: vec!["order_key".into()],
    };
    (source, target)
}
fn meanings(p: &[(&str, &str)]) -> BTreeMap<String, String> {
    p.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
fn evidence(total: &str) -> Evidence {
    Evidence {
        source_meanings: meanings(&[
            ("order_id", "Order identifier"),
            ("cents", "Order grand total in US cents"),
            ("tax_cents", "Sales tax in US cents"),
        ]),
        target_meanings: meanings(&[
            ("order_key", "Order identifier"),
            ("tax", "Sales tax in US cents"),
            ("total", total),
        ]),
        context: "Synthetic billing migration".into(),
    }
}
fn response(choice: &str, p1: f64, p2: f64, review: f64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "model": MODEL,
        "answers": {"pick": {"type": "choice", "choice": choice, "confidence": 0.93,
            "probabilities": {"option_1": p1, "option_2": p2, "review": review}}},
        "usage": {"input_tokens": 512, "output_tokens": 40}
    }))
    .unwrap_or_default()
}
fn prepared() -> Result<Vec<u8>> {
    let (s, t) = ambiguous();
    match suggest::prepare(&s, &t, &evidence("Grand total in US cents"))? {
        Prepared::RequestPrepared {
            payload,
            candidates,
            model,
            ..
        } => {
            assert_eq!(candidates.len(), 2);
            assert_eq!(model, MODEL);
            Ok(payload)
        }
        other => panic!("expected a request, got {other:?}"),
    }
}

#[test]
fn single_structural_candidate_needs_no_request() -> Result<()> {
    let (mut s, mut t) = ambiguous();
    s.fields[2].data_type = DataType::Utf8;
    t.fields[1].data_type = DataType::Utf8;
    let Prepared::UniqueCandidate { mapping } = suggest::prepare(&s, &t, &evidence("x"))? else {
        panic!("expected the unique candidate");
    };
    assert_eq!(mapping.fields.len(), 3);
    Ok(())
}

#[test]
fn incompatible_schemas_are_refused() {
    let (s, mut t) = ambiguous();
    t.fields[2].data_type = DataType::Bool;
    assert!(suggest::prepare(&s, &t, &evidence("x")).is_err());
}

#[test]
fn confident_unique_choice_is_accepted_with_provenance() -> Result<()> {
    let (s, t) = ambiguous();
    let request = prepared()?;
    let response = response("option_2", 0.02, 0.96, 0.02);
    let admitted = suggest::admit(
        &s,
        &t,
        &evidence("Grand total in US cents"),
        &request,
        &response,
    )?;
    let Admitted::Accepted {
        candidate_id,
        mapping,
        provenance,
    } = admitted
    else {
        panic!("expected acceptance");
    };
    assert_eq!(candidate_id, "option_2");
    let total = mapping.fields.iter().find(|f| f.target == "total");
    assert_eq!(total.map(|f| f.source.as_str()), Some("cents"));
    assert_eq!(provenance.response_model, MODEL);
    assert_eq!(provenance.input_tokens, 512);
    Ok(())
}

#[test]
fn abstention_and_weak_margins_require_review() -> Result<()> {
    let (s, t) = ambiguous();
    let request = prepared()?;
    let ev = evidence("Grand total in US cents");
    for r in [
        response("review", 0.1, 0.1, 0.8),
        response("option_2", 0.38, 0.6, 0.02),
        response("option_2", 0.1, 0.89, 0.01),
    ] {
        let admitted = suggest::admit(&s, &t, &ev, &request, &r)?;
        assert!(
            matches!(admitted, Admitted::ReviewRequired { ref candidates, .. } if candidates.len() == 2),
            "{admitted:?}"
        );
    }
    Ok(())
}

#[test]
fn changed_evidence_or_schema_invalidates_the_saved_request() -> Result<()> {
    let (s, mut t) = ambiguous();
    let request = prepared()?;
    let r = response("option_2", 0.02, 0.96, 0.02);
    assert!(suggest::admit(&s, &t, &evidence("Total incl. tax"), &request, &r).is_err());
    t.relation = "purchases_v2".into();
    assert!(suggest::admit(&s, &t, &evidence("Grand total in US cents"), &request, &r).is_err());
    Ok(())
}

#[test]
fn malformed_foreign_or_oversized_responses_are_refused() -> Result<()> {
    let (s, t) = ambiguous();
    let request = prepared()?;
    let ev = evidence("Grand total in US cents");
    let mut wrong_model: serde_json::Value =
        serde_json::from_slice(&response("option_2", 0.02, 0.96, 0.02))?;
    wrong_model["model"] = "jev-1.13.0".into();
    let mut unknown = wrong_model.clone();
    unknown["model"] = MODEL.into();
    unknown["meta"] = serde_json::json!({"usd_spent": 0.1});
    let invented = response("option_9", 0.02, 0.96, 0.02);
    let huge = vec![b' '; suggest::MAX_RESPONSE_BYTES + 1];
    for bad in [
        serde_json::to_vec(&wrong_model)?,
        serde_json::to_vec(&unknown)?,
        invented,
        huge,
        b"not json".to_vec(),
    ] {
        assert!(suggest::admit(&s, &t, &ev, &request, &bad).is_err());
    }
    Ok(())
}

#[test]
fn gateway_cost_is_recorded() -> Result<()> {
    let (s, t) = ambiguous();
    let request = prepared()?;
    let mut r: serde_json::Value = serde_json::from_slice(&response("option_2", 0.02, 0.96, 0.02))?;
    r["usage"]["cost"] = 0.0000215.into();
    let admitted = suggest::admit(
        &s,
        &t,
        &evidence("Grand total in US cents"),
        &request,
        &serde_json::to_vec(&r)?,
    )?;
    let Admitted::Accepted { provenance, .. } = admitted else {
        panic!("expected acceptance");
    };
    assert_eq!(provenance.cost, Some(0.0000215));
    Ok(())
}

#[test]
fn outputs_are_never_overwritten() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("dm-suggest-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("request.json");
    let _ = std::fs::remove_file(&path);
    suggest::save_bytes(&path, b"first")?;
    assert!(suggest::save_bytes(&path, b"second").is_err());
    assert_eq!(suggest::read_bounded(&path, 16)?, b"first");
    assert!(suggest::read_bounded(&path, 3).is_err());
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
