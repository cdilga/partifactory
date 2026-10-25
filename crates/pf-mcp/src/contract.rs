//! Only schema-derived field tables and error lists are generated; prose remains hand-written.
use serde_json::Value;
fn kind(schema: &Value) -> String {
    if let Some(reference) = schema["$ref"].as_str() {
        return reference.rsplit('/').next().unwrap_or("JSON").into();
    }
    if let Some(values) = schema["enum"].as_array() {
        return values
            .iter()
            .filter(|v| !v.is_null())
            .map(|v| {
                format!(
                    "`{}`",
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                )
            })
            .collect::<Vec<_>>()
            .join(" \\| ");
    }
    if let Some(alternatives) = schema["anyOf"]
        .as_array()
        .or_else(|| schema["oneOf"].as_array())
    {
        return alternatives
            .iter()
            .filter(|v| v["type"] != "null")
            .map(kind)
            .collect::<Vec<_>>()
            .join(" \\| ");
    }
    if let Some(types) = schema["type"].as_array() {
        return types
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| *t != "null")
            .collect::<Vec<_>>()
            .join(" \\| ");
    }
    match schema["type"].as_str() {
        Some("array") => format!("[{}]", kind(&schema["items"])),
        Some("object") => "object".into(),
        Some(t) => t.into(),
        None => "JSON".into(),
    }
}
pub fn table(definition: &pf_ops::Definition) -> String {
    let mut out =
        "| Field | Type | Req | Default | Meaning |\n| --- | --- | --- | --- | --- |\n".to_string();
    let required = definition.input["required"].as_array();
    let notes = pf_ops::guide::fields(definition.name);
    for note in notes {
        assert!(
            note.fields
                .iter()
                .all(|field| definition.input["properties"].get(field).is_some()),
            "guide field is absent from registry schema"
        );
        let descriptions = note
            .fields
            .iter()
            .filter_map(|field| definition.input["properties"][field]["description"].as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let meaning = if descriptions.is_empty() {
            note.meaning.to_string()
        } else {
            format!("{} {}", note.meaning, descriptions)
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            note.label, note.kind, note.required, note.default, meaning
        ));
    }
    for (name, schema) in definition.input["properties"]
        .as_object()
        .expect("input object")
    {
        if notes
            .iter()
            .any(|note| note.fields.contains(&name.as_str()))
        {
            continue;
        }
        let description = schema["description"]
            .as_str()
            .unwrap_or("")
            .replace('|', "\\|")
            .replace('\n', " ");
        out.push_str(&format!(
            "| `{name}` | {} | {} | {} | {} |\n",
            kind(schema),
            if required.is_some_and(|r| r.iter().any(|v| v == name)) {
                "yes"
            } else {
                "no"
            },
            schema
                .get("default")
                .filter(|v| !v.is_null())
                .map(|v| format!("`{v}`"))
                .unwrap_or_else(|| "—".into()),
            description
        ));
    }
    out
}
pub fn regenerate(document: &str) -> String {
    let mut result = document.to_string();
    for definition in pf_ops::definitions() {
        let marker = format!("<!-- pf:op {} -->", definition.name);
        let start = result
            .find(&marker)
            .expect("every registry op has a contract section");
        let relative = &result[start + marker.len()..];
        let end = relative
            .find("<!-- pf:op ")
            .into_iter()
            .chain(relative.find("\n## "))
            .min()
            .map(|n| start + marker.len() + n)
            .unwrap_or(result.len());
        let section = &result[start..end];
        let table_start = section.find("| Field |").expect("field table");
        let table_end = section[table_start..]
            .lines()
            .take_while(|l| l.starts_with('|'))
            .map(|l| l.len() + 1)
            .sum::<usize>()
            + table_start;
        let mut section = section.to_string();
        section.replace_range(table_start..table_end, &table(&definition));
        let error_start = section.find("Errors: ").expect("error line");
        let error_end = section[error_start..]
            .find('\n')
            .map(|n| error_start + n)
            .unwrap_or(section.len());
        let errors = format!(
            "Errors: {}.",
            pf_ops::catalogue::for_op(definition.name)
                .iter()
                .map(|code| format!("`{code}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        section.replace_range(error_start..error_end, &errors);
        result.replace_range(start..end, &section);
    }
    result
}
#[cfg(test)]
mod tests {
    #[test]
    fn generated_sections_are_current() {
        let current = super::super::discovery::CONTRACT;
        assert_eq!(
            super::regenerate(current),
            current,
            "Build examples with the standard workspace feature set and run target/debug/examples/regenerate_contract"
        );
    }
    #[test]
    fn every_op_error_is_in_the_catalogue() {
        for op in pf_ops::definitions() {
            assert!(!pf_ops::catalogue::for_op(op.name).is_empty());
            for code in pf_ops::catalogue::for_op(op.name) {
                assert!(pf_ops::catalogue::CODES.contains(code));
            }
        }
    }
}

#[cfg(test)]
mod conformance_tests {
    use serde_json::Value;
    #[test]
    fn every_documented_input_and_output_parses_against_its_schema() {
        let contract = crate::discovery::CONTRACT;
        for op in pf_ops::definitions() {
            let marker = format!("<!-- pf:op {} -->", op.name);
            let tail = contract.split_once(&marker).unwrap().1;
            let end = tail
                .find("<!-- pf:op ")
                .into_iter()
                .chain(tail.find("\n## "))
                .min()
                .unwrap_or(tail.len());
            let section = &tail[..end];
            let schema = jsonschema::validator_for(&op.input).unwrap();
            let output_schema = serde_json::json!({"type":"object","additionalProperties":true});
            let output = jsonschema::validator_for(&output_schema).unwrap();
            let mut count = 0;
            for block in section.split("```json\n").skip(1) {
                let example = block.split_once("```").unwrap().0;
                let (input, result) = example.split_once('→').expect("input and output example");
                let input: Value = serde_json::from_str(input.trim()).unwrap();
                let result: Value = serde_json::from_str(result.trim()).unwrap();
                schema
                    .validate(&input)
                    .unwrap_or_else(|e| panic!("{} input example: {e}", op.name));
                output
                    .validate(&result)
                    .unwrap_or_else(|e| panic!("{} output example: {e}", op.name));
                count += 1;
            }
            assert!(count > 0, "{} has an example", op.name);
        }
    }
    #[test]
    fn scope_and_error_tables_match_the_registry() {
        let contract = crate::discovery::CONTRACT;
        for op in pf_ops::definitions() {
            let row = contract
                .lines()
                .find(|l| l.starts_with("|") && l.contains(&format!("`{}`", op.scope)))
                .unwrap();
            assert!(row.contains(op.name), "{} scope row", op.name);
        }
        let table = contract
            .split_once("| Code | HTTP |")
            .unwrap()
            .1
            .split_once("\n## ")
            .unwrap()
            .0;
        for code in pf_ops::catalogue::CODES {
            let row = table
                .lines()
                .find(|l| l.contains(&format!("`{code}`")))
                .unwrap_or_else(|| panic!("undocumented code {code}"));
            let status = pf_model::PfError::new(code, "").http_status();
            assert!(
                row.contains(&format!(
                    "| {status} | {} |",
                    pf_ops::catalogue::exit_code(code)
                )),
                "{code}: {row}"
            );
        }
    }
    #[test]
    fn all_catalogue_errors_have_mcp_mapping() {
        for code in pf_ops::catalogue::CODES {
            let error = pf_model::PfError::new(code, "fixture");
            let mapped = crate::error_body(&error, "https://parts.example.com");
            assert_eq!(mapped["error"]["code"], *code);
            assert!(
                mapped["error"]["hint"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            );
            assert!(mapped["error"]["docs"].as_str().unwrap().ends_with(code));
            assert!((2..=7).contains(&pf_ops::catalogue::exit_code(code)));
        }
    }
    #[test]
    fn rust_field_description_changes_make_the_contract_stale() {
        let mut op = pf_ops::definitions().remove(0);
        let before = super::table(&op);
        op.input["properties"]["title"]["description"] =
            serde_json::json!("changed field description");
        assert_ne!(super::table(&op), before);
    }
}

#[cfg(test)]
mod snapshots {
    #[test]
    fn tools_and_annotations_match_reviewed_snapshot() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tools.snapshot.json");
        let expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path)
                .expect("run regenerate_contract to create tools snapshot"),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(crate::tools()).unwrap(),
            expected,
            "review schema changes and run regenerate_contract"
        );
    }
}
