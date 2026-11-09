use std::{collections::BTreeSet, path::Path};

use anyhow::{Context, Result, anyhow, bail};
use nokturn_core::ir::{
    AdditionalBehavior, Field, HeaderIr, LiteralValue, Model, ModelId, ModelKind, Operation,
    ParameterIr, ParameterLocation, PrimitiveKind, RequestBodyIr, ResponseIr, ResponseStatus,
    TypeExpr,
};
use nokturn_core::lower_spec;
use nokturn_core::lowering::{LoweredApi, LoweringDiagnostic};
use nokturn_core::utils::HttpMethod;
use tokio::fs;

use crate::Config;

pub async fn generate_sdk(config: &Config, spec: &str, ext: &str, output_dir: &Path) -> Result<()> {
    let lowered = lower_spec(spec, ext)?;
    ensure_no_diagnostics(&lowered.diagnostics)?;
    ensure_codegen_support(&lowered)?;

    let models = select_models(&lowered, config)?;
    let cargo_toml = render_cargo_toml(config);
    let lib_rs = render_lib_rs(config);
    let models_rs = render_models_rs(&models)?;
    let operations_rs = render_operations_rs(&lowered.operations)?;
    let client_rs = render_client_rs(&lowered.operations)?;

    let src_dir = output_dir.join("src");
    fs::create_dir_all(&src_dir)
        .await
        .with_context(|| format!("failed to create output directory {}", src_dir.display()))?;
    fs::write(output_dir.join("Cargo.toml"), cargo_toml)
        .await
        .with_context(|| "failed to write generated Cargo.toml")?;
    fs::write(src_dir.join("lib.rs"), lib_rs)
        .await
        .with_context(|| "failed to write generated src/lib.rs")?;
    fs::write(src_dir.join("models.rs"), models_rs)
        .await
        .with_context(|| "failed to write generated src/models.rs")?;
    fs::write(src_dir.join("operations.rs"), operations_rs)
        .await
        .with_context(|| "failed to write generated src/operations.rs")?;
    fs::write(src_dir.join("client.rs"), client_rs)
        .await
        .with_context(|| "failed to write generated src/client.rs")?;

    Ok(())
}

fn ensure_no_diagnostics(diagnostics: &[LoweringDiagnostic]) -> Result<()> {
    if diagnostics.is_empty() {
        return Ok(());
    }

    let details = diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "{:?} {:?}: {}",
                diagnostic.kind, diagnostic.target, diagnostic.reference
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    bail!("lowering emitted diagnostics; generation aborted:\n{details}");
}

fn ensure_codegen_support(lowered: &LoweredApi) -> Result<()> {
    for operation in &lowered.operations {
        ensure_operation_codegen_support(operation)?;
    }

    Ok(())
}

fn ensure_operation_codegen_support(operation: &Operation) -> Result<()> {
    if let Some(body) = operation.request_body.as_ref() {
        if body.contents.len() > 1 {
            bail!(
                "operation `{}` has multiple request body media types; code generation currently supports exactly one JSON request body media type",
                operation.id
            );
        }

        if let Some(media) = body.contents.first()
            && !is_json_media_type(&media.media_type)
        {
            bail!(
                "operation `{}` uses unsupported request body media type `{}`; code generation currently supports JSON request bodies only",
                operation.id,
                media.media_type
            );
        }
    }

    Ok(())
}

fn is_json_media_type(media_type: &str) -> bool {
    let normalized = media_type.trim().to_ascii_lowercase();
    normalized == "application/json" || normalized.ends_with("+json")
}

fn select_models<'a>(
    lowered: &'a LoweredApi,
    config: &Config,
) -> Result<Vec<(&'a str, &'a Model)>> {
    let available = lowered
        .models
        .keys()
        .map(|id| id.0.as_str())
        .collect::<BTreeSet<_>>();

    if let Some(include_only) = config.include_only.as_ref() {
        let missing = include_only
            .iter()
            .filter(|name| !available.contains(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            bail!(
                "include_only references unknown models: {}",
                missing.join(", ")
            );
        }
    }

    let excluded = config
        .exclude
        .as_ref()
        .map(|items| items.iter().map(String::as_str).collect::<BTreeSet<_>>())
        .unwrap_or_default();

    let root_names = match config.include_only.as_ref() {
        Some(include_only) => include_only.iter().cloned().collect::<BTreeSet<_>>(),
        None => available
            .iter()
            .filter(|name| !excluded.contains(**name))
            .map(|name| (*name).to_string())
            .collect::<BTreeSet<_>>(),
    };

    let mut selected = BTreeSet::new();
    let mut stack = root_names.iter().cloned().collect::<Vec<_>>();

    while let Some(name) = stack.pop() {
        if !selected.insert(name.clone()) {
            continue;
        }

        let model = lowered
            .models
            .get(&ModelId(name.clone()))
            .ok_or_else(|| anyhow!("lowered models missing expected model: {name}"))?;

        for reference in referenced_models(model) {
            if excluded.contains(reference.as_str()) {
                bail!("excluded model `{reference}` is required by selected model `{name}`");
            }
            if !selected.contains(reference.as_str()) {
                stack.push(reference);
            }
        }
    }

    let mut models = lowered
        .models
        .iter()
        .filter_map(|(id, model)| selected.contains(&id.0).then_some((id.0.as_str(), model)))
        .collect::<Vec<_>>();
    models.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(models)
}

fn referenced_models(model: &Model) -> Vec<String> {
    let mut refs = BTreeSet::new();

    match &model.kind {
        ModelKind::Struct { fields, additional } => {
            for field in fields {
                collect_type_refs(&field.ty, &mut refs);
            }
            collect_additional_refs(additional, &mut refs);
        }
        ModelKind::Alias(ty) => collect_type_refs(ty, &mut refs),
        ModelKind::Enum { .. } | ModelKind::Union { .. } => {}
    }

    refs.into_iter().collect()
}

fn collect_additional_refs(additional: &AdditionalBehavior, refs: &mut BTreeSet<String>) {
    if let AdditionalBehavior::Typed(ty) = additional {
        collect_type_refs(ty, refs);
    }
}

fn collect_type_refs(ty: &TypeExpr, refs: &mut BTreeSet<String>) {
    match ty {
        TypeExpr::Reference(ModelId(name)) => {
            refs.insert(name.clone());
        }
        TypeExpr::Array(inner) | TypeExpr::Optional(inner) | TypeExpr::Nullable(inner) => {
            collect_type_refs(inner, refs);
        }
        TypeExpr::Map { value, additional } => {
            collect_type_refs(value, refs);
            collect_additional_refs(additional, refs);
        }
        TypeExpr::OneOf(items) | TypeExpr::AnyOf(items) | TypeExpr::AllOf(items) => {
            for item in items {
                collect_type_refs(item, refs);
            }
        }
        TypeExpr::DiscriminatedUnion { mapping, .. } => {
            for item in mapping.values() {
                collect_type_refs(item, refs);
            }
        }
        TypeExpr::Primitive(_) | TypeExpr::Literal(_) => {}
    }
}

fn render_cargo_toml(config: &Config) -> String {
    let mut doc = toml::Table::new();

    let mut package = toml::Table::new();
    package.insert("name".to_string(), toml::Value::String(config.crate_name.clone()));
    package.insert("version".to_string(), toml::Value::String(config.version.clone()));
    package.insert(
        "edition".to_string(),
        toml::Value::String(config.edition.clone().unwrap_or_else(|| "2021".to_string())),
    );
    package.insert(
        "description".to_string(),
        toml::Value::String(config.description.clone()),
    );
    package.insert(
        "authors".to_string(),
        toml::Value::Array(
            config
                .authors
                .iter()
                .map(|a| toml::Value::String(a.clone()))
                .collect(),
        ),
    );
    package.insert(
        "keywords".to_string(),
        toml::Value::Array(
            config
                .keywords
                .iter()
                .map(|k| toml::Value::String(k.clone()))
                .collect(),
        ),
    );
    doc.insert("package".to_string(), toml::Value::Table(package));

    let mut dependencies = toml::Table::new();
    let mut reqwest = toml::Table::new();
    reqwest.insert("version".to_string(), toml::Value::String("0.13".to_string()));
    reqwest.insert(
        "features".to_string(),
        toml::Value::Array(vec![toml::Value::String("json".to_string())]),
    );
    dependencies.insert("reqwest".to_string(), toml::Value::Table(reqwest));

    let mut serde = toml::Table::new();
    serde.insert("version".to_string(), toml::Value::String("1".to_string()));
    serde.insert(
        "features".to_string(),
        toml::Value::Array(vec![toml::Value::String("derive".to_string())]),
    );
    dependencies.insert("serde".to_string(), toml::Value::Table(serde));

    dependencies.insert(
        "serde_json".to_string(),
        toml::Value::String("1".to_string()),
    );
    doc.insert(
        "dependencies".to_string(),
        toml::Value::Table(dependencies),
    );

    let mut metadata = toml::Table::new();
    let mut nokturn = toml::Table::new();
    nokturn.insert(
        "lib-status".to_string(),
        toml::Value::String(config.lib_status.clone()),
    );
    nokturn.insert(
        "api-url".to_string(),
        toml::Value::String(config.api_url.clone()),
    );
    metadata.insert("nokturn".to_string(), toml::Value::Table(nokturn));
    doc.insert(
        "package".to_string(),
        toml::Value::Table({
            let mut pkg = toml::Table::new();
            pkg.insert("metadata".to_string(), toml::Value::Table(metadata));
            pkg
        }),
    );

    let mut root = toml::Table::new();
    root.clone_from(&doc);

    // Rebuild with correct package.metadata structure
    let mut final_doc = toml::Table::new();

    let mut final_package = toml::Table::new();
    final_package.insert("name".to_string(), toml::Value::String(config.crate_name.clone()));
    final_package.insert("version".to_string(), toml::Value::String(config.version.clone()));
    final_package.insert(
        "edition".to_string(),
        toml::Value::String(config.edition.clone().unwrap_or_else(|| "2021".to_string())),
    );
    final_package.insert(
        "description".to_string(),
        toml::Value::String(config.description.clone()),
    );
    final_package.insert(
        "authors".to_string(),
        toml::Value::Array(
            config
                .authors
                .iter()
                .map(|a| toml::Value::String(a.clone()))
                .collect(),
        ),
    );
    final_package.insert(
        "keywords".to_string(),
        toml::Value::Array(
            config
                .keywords
                .iter()
                .map(|k| toml::Value::String(k.clone()))
                .collect(),
        ),
    );

    let mut final_metadata = toml::Table::new();
    let mut final_nokturn = toml::Table::new();
    final_nokturn.insert(
        "lib-status".to_string(),
        toml::Value::String(config.lib_status.clone()),
    );
    final_nokturn.insert(
        "api-url".to_string(),
        toml::Value::String(config.api_url.clone()),
    );
    final_metadata.insert("nokturn".to_string(), toml::Value::Table(final_nokturn));
    final_package.insert(
        "metadata".to_string(),
        toml::Value::Table(final_metadata),
    );

    final_doc.insert("package".to_string(), toml::Value::Table(final_package));

    let mut final_deps = toml::Table::new();
    let mut final_reqwest = toml::Table::new();
    final_reqwest.insert("version".to_string(), toml::Value::String("0.13".to_string()));
    final_reqwest.insert(
        "features".to_string(),
        toml::Value::Array(vec![toml::Value::String("json".to_string())]),
    );
    final_deps.insert("reqwest".to_string(), toml::Value::Table(final_reqwest));

    let mut final_serde = toml::Table::new();
    final_serde.insert("version".to_string(), toml::Value::String("1".to_string()));
    final_serde.insert(
        "features".to_string(),
        toml::Value::Array(vec![toml::Value::String("derive".to_string())]),
    );
    final_deps.insert("serde".to_string(), toml::Value::Table(final_serde));

    final_deps.insert(
        "serde_json".to_string(),
        toml::Value::String("1".to_string()),
    );
    final_doc.insert(
        "dependencies".to_string(),
        toml::Value::Table(final_deps),
    );

    toml::to_string(&toml::Value::Table(final_doc)).expect("failed to serialize Cargo.toml")
}

fn render_lib_rs(config: &Config) -> String {
    format!(
        "pub mod client;\npub mod models;\npub mod operations;\n\npub use client::{{Client, ClientError}};\n\npub const API_BASE_URL: &str = \"{}\";\n",
        escape_string(&config.api_url)
    )
}

fn render_models_rs(models: &[(&str, &Model)]) -> Result<String> {
    let mut out =
        String::from("use serde::{Deserialize, Serialize};\nuse std::collections::BTreeMap;\n\n");

    for (name, model) in models {
        render_model(&mut out, name, model)?;
        out.push('\n');
    }

    Ok(out)
}

fn render_model(out: &mut String, name: &str, model: &Model) -> Result<()> {
    if let Some(docs) = model.docs.as_deref() {
        push_doc_comment(out, docs, 0);
    }

    let rust_name = rust_type_name(name);
    match &model.kind {
        ModelKind::Struct { fields, .. } => {
            out.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
            out.push_str(&format!("pub struct {rust_name} {{\n"));
            for field in fields {
                render_field(out, field)?;
            }
            out.push_str("}\n");
        }
        ModelKind::Alias(ty) => {
            let rendered = render_type_expr(ty)?;
            out.push_str(&format!("pub type {rust_name} = {rendered};\n"));
        }
        ModelKind::Enum { .. } => bail!("enum code generation is not implemented for `{name}`"),
        ModelKind::Union { .. } => bail!("union code generation is not implemented for `{name}`"),
    }

    Ok(())
}

fn render_field(out: &mut String, field: &Field) -> Result<()> {
    if let Some(description) = field.metadata.description.as_deref() {
        push_doc_comment(out, description, 4);
    }

    let rust_name = rust_field_name(&field.json_name);
    let ty = render_field_type(field)?;
    let rename = (rust_name != field.json_name).then_some(field.json_name.as_str());

    if let Some(rename) = rename {
        out.push_str(&format!(
            "    #[serde(rename = \"{}\")]\n",
            escape_string(rename)
        ));
    }
    out.push_str(&format!("    pub {rust_name}: {ty},\n"));
    Ok(())
}

fn render_field_type(field: &Field) -> Result<String> {
    let rendered = render_type_expr(&field.ty)?;
    if field.required {
        Ok(rendered)
    } else {
        Ok(format!("Option<{rendered}>"))
    }
}

fn render_type_expr(ty: &TypeExpr) -> Result<String> {
    match ty {
        TypeExpr::Primitive(kind) => Ok(render_primitive(kind)),
        TypeExpr::Literal(literal) => Ok(render_literal_type(literal)),
        TypeExpr::Array(inner) => Ok(format!("Vec<{}>", render_type_expr(inner)?)),
        TypeExpr::Map { value, .. } => {
            Ok(format!("BTreeMap<String, {}>", render_type_expr(value)?))
        }
        TypeExpr::Optional(inner) | TypeExpr::Nullable(inner) => {
            Ok(format!("Option<{}>", render_type_expr(inner)?))
        }
        TypeExpr::Reference(ModelId(name)) => Ok(format!("{}{}", "", rust_type_name(name))),
        TypeExpr::OneOf(_) => bail!("oneOf code generation is not implemented"),
        TypeExpr::AnyOf(_) => bail!("anyOf code generation is not implemented"),
        TypeExpr::AllOf(_) => bail!("allOf code generation is not implemented"),
        TypeExpr::DiscriminatedUnion { .. } => {
            bail!("discriminated union code generation is not implemented")
        }
    }
}

fn render_type_expr_in_module(ty: &TypeExpr, model_prefix: &str) -> Result<String> {
    match ty {
        TypeExpr::Primitive(kind) => Ok(render_primitive(kind)),
        TypeExpr::Literal(literal) => Ok(render_literal_type(literal)),
        TypeExpr::Array(inner) => Ok(format!(
            "Vec<{}>",
            render_type_expr_in_module(inner, model_prefix)?
        )),
        TypeExpr::Map { value, .. } => Ok(format!(
            "BTreeMap<String, {}>",
            render_type_expr_in_module(value, model_prefix)?
        )),
        TypeExpr::Optional(inner) | TypeExpr::Nullable(inner) => Ok(format!(
            "Option<{}>",
            render_type_expr_in_module(inner, model_prefix)?
        )),
        TypeExpr::Reference(ModelId(name)) => Ok(format!("{model_prefix}{}", rust_type_name(name))),
        TypeExpr::OneOf(_) => bail!("oneOf code generation is not implemented"),
        TypeExpr::AnyOf(_) => bail!("anyOf code generation is not implemented"),
        TypeExpr::AllOf(_) => bail!("allOf code generation is not implemented"),
        TypeExpr::DiscriminatedUnion { .. } => {
            bail!("discriminated union code generation is not implemented")
        }
    }
}

fn render_literal_type(literal: &LiteralValue) -> String {
    match literal {
        LiteralValue::Integer(_) => "i64".to_string(),
        LiteralValue::Number(_) => "f64".to_string(),
        LiteralValue::Boolean(_) => "bool".to_string(),
        LiteralValue::String(_) => "String".to_string(),
    }
}

fn render_primitive(kind: &PrimitiveKind) -> String {
    match kind {
        PrimitiveKind::Boolean => "bool".to_string(),
        PrimitiveKind::Integer32 => "i32".to_string(),
        PrimitiveKind::Integer64 => "i64".to_string(),
        PrimitiveKind::NumberFloat => "f32".to_string(),
        PrimitiveKind::NumberDouble => "f64".to_string(),
        PrimitiveKind::String
        | PrimitiveKind::StringDate
        | PrimitiveKind::StringDateTime
        | PrimitiveKind::StringPassword
        | PrimitiveKind::StringUuid => "String".to_string(),
        PrimitiveKind::StringByte | PrimitiveKind::StringBinary => "Vec<u8>".to_string(),
    }
}

fn rust_type_name(name: &str) -> String {
    let mut out = String::new();
    let mut capitalize = true;

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if capitalize {
                out.extend(ch.to_uppercase());
            } else {
                out.push(ch);
            }
            capitalize = false;
        } else {
            capitalize = true;
        }
    }

    if out.is_empty() {
        "GeneratedType".to_string()
    } else if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("T{out}")
    } else {
        out
    }
}

fn rust_field_name(name: &str) -> String {
    let mut out = String::new();
    let mut previous_is_separator = true;

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() {
                if !previous_is_separator && !out.is_empty() {
                    out.push('_');
                }
                out.push(ch.to_ascii_lowercase());
            } else {
                out.push(ch);
            }
            previous_is_separator = false;
        } else {
            if !previous_is_separator && !out.is_empty() {
                out.push('_');
            }
            previous_is_separator = true;
        }
    }

    let trimmed = out.trim_matches('_');
    let normalized = if trimmed.is_empty() { "field" } else { trimmed };
    let normalized = if normalized
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        format!("field_{normalized}")
    } else {
        normalized.to_string()
    };

    match normalized.as_str() {
        "type" | "match" | "ref" | "self" | "super" | "crate" | "mod" | "enum" | "struct"
        | "fn" | "trait" | "impl" | "move" | "async" | "await" => format!("r#{normalized}"),
        _ => normalized,
    }
}

fn push_doc_comment(out: &mut String, docs: &str, indent: usize) {
    let prefix = " ".repeat(indent);
    for line in docs.lines() {
        out.push_str(&format!("{prefix}/// {}\n", line.trim()));
    }
}

fn escape_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\"', "\\\"")
}

fn render_operations_rs(operations: &[Operation]) -> Result<String> {
    let mut out = String::from(
        "use serde::{Deserialize, Serialize};\nuse std::collections::BTreeMap;\n\n#[derive(Debug, Clone, Copy)]\npub struct OperationMetadata {\n    pub id: &'static str,\n    pub method: &'static str,\n    pub path: &'static str,\n}\n\n",
    );

    for operation in operations {
        render_operation_module(&mut out, operation)?;
        out.push('\n');
    }

    Ok(out)
}

fn render_operation_module(out: &mut String, operation: &Operation) -> Result<()> {
    let module_name = rust_field_name(&operation.id);
    let path_params = operation
        .params
        .iter()
        .filter(|param| param.location == ParameterLocation::Path)
        .collect::<Vec<_>>();
    let query_params = operation
        .params
        .iter()
        .filter(|param| param.location == ParameterLocation::Query)
        .collect::<Vec<_>>();
    let header_params = operation
        .params
        .iter()
        .filter(|param| param.location == ParameterLocation::Header)
        .collect::<Vec<_>>();
    let cookie_params = operation
        .params
        .iter()
        .filter(|param| param.location == ParameterLocation::Cookie)
        .collect::<Vec<_>>();

    out.push_str(&format!("pub mod {module_name} {{\n"));
    out.push_str("    use super::OperationMetadata;\n");
    out.push_str("    use serde::{Deserialize, Serialize};\n");
    out.push_str("    use std::collections::BTreeMap;\n\n");

    if let Some(docs) = operation.docs.as_deref().or(operation.summary.as_deref()) {
        push_doc_comment(out, docs, 4);
    }

    out.push_str("    pub const META: OperationMetadata = OperationMetadata {\n");
    out.push_str(&format!(
        "        id: \"{}\",\n        method: \"{}\",\n        path: \"{}\",\n",
        escape_string(&operation.id),
        http_method_name(operation.method),
        escape_string(&operation.path)
    ));
    out.push_str("    };\n\n");

    if !path_params.is_empty() {
        render_parameter_group(out, "PathParams", &path_params)?;
    }
    if !query_params.is_empty() {
        render_parameter_group(out, "QueryParams", &query_params)?;
    }
    if !header_params.is_empty() {
        render_parameter_group(out, "HeaderParams", &header_params)?;
    }
    if !cookie_params.is_empty() {
        render_parameter_group(out, "CookieParams", &cookie_params)?;
    }
    if let Some(body) = operation.request_body.as_ref() {
        render_request_body(out, body)?;
    }
    render_responses(out, &operation.responses)?;

    out.push_str("    #[derive(Debug, Clone, Serialize, Deserialize)]\n");
    out.push_str("    pub struct Operation {\n");
    if !path_params.is_empty() {
        out.push_str("        pub path_params: PathParams,\n");
    }
    if !query_params.is_empty() {
        out.push_str("        pub query_params: QueryParams,\n");
    }
    if !header_params.is_empty() {
        out.push_str("        pub header_params: HeaderParams,\n");
    }
    if !cookie_params.is_empty() {
        out.push_str("        pub cookie_params: CookieParams,\n");
    }
    if let Some(body) = operation.request_body.as_ref() {
        if body.required {
            out.push_str("        pub body: RequestBody,\n");
        } else {
            out.push_str("        pub body: Option<RequestBody>,\n");
        }
    }
    out.push_str("    }\n\n");

    out.push_str("    impl Operation {\n");
    out.push_str("        pub const META: OperationMetadata = META;\n");
    out.push_str("        pub fn path(&self) -> String {\n");
    out.push_str(&format!(
        "            let mut path = \"{}\".to_string();\n",
        escape_string(&operation.path)
    ));
    for param in &path_params {
        let field_name = rust_field_name(&param.name);
        out.push_str(&format!(
            "            path = path.replace(\"{{{}}}\", &self.path_params.{}.to_string());\n",
            escape_string(&param.name),
            field_name
        ));
    }
    out.push_str("            path\n");
    out.push_str("        }\n");
    if operation
        .request_body
        .as_ref()
        .is_some_and(|body| !body.required)
    {
        out.push_str("        pub fn with_body(mut self, body: RequestBody) -> Self {\n");
        out.push_str("            self.body = Some(body);\n");
        out.push_str("            self\n");
        out.push_str("        }\n");
    }
    out.push_str("    }\n");

    out.push_str("}\n");
    Ok(())
}

fn render_parameter_group(out: &mut String, name: &str, params: &[&ParameterIr]) -> Result<()> {
    out.push_str("    #[derive(Debug, Clone, Default, Serialize, Deserialize)]\n");
    out.push_str(&format!("    pub struct {name} {{\n"));
    for param in params {
        if let Some(description) = param.description.as_deref() {
            push_doc_comment(out, description, 8);
        }
        let field_name = rust_field_name(&param.name);
        let ty = render_parameter_type(param)?;
        if field_name != param.name {
            out.push_str(&format!(
                "        #[serde(rename = \"{}\")]\n",
                escape_string(&param.name)
            ));
        }
        out.push_str(&format!("        pub {field_name}: {ty},\n"));
    }
    out.push_str("    }\n\n");
    Ok(())
}

fn render_parameter_type(param: &ParameterIr) -> Result<String> {
    let ty = render_type_expr_in_module(&param.ty, "crate::models::")?;
    if param.required {
        Ok(ty)
    } else {
        Ok(format!("Option<{ty}>"))
    }
}

fn render_request_body(out: &mut String, body: &RequestBodyIr) -> Result<()> {
    if let Some(description) = body.description.as_deref() {
        push_doc_comment(out, description, 4);
    }

    if body.contents.len() == 1 {
        let ty = render_type_expr_in_module(&body.contents[0].schema, "crate::models::")?;
        out.push_str(&format!("    pub type RequestBody = {ty};\n\n"));
    } else {
        out.push_str("    #[derive(Debug, Clone, Serialize, Deserialize)]\n");
        out.push_str("    pub enum RequestBody {\n");
        for media in &body.contents {
            let variant = rust_type_name(&media.media_type);
            let ty = render_type_expr_in_module(&media.schema, "crate::models::")?;
            out.push_str(&format!("        {variant}({ty}),\n"));
        }
        out.push_str("    }\n\n");
    }
    Ok(())
}

fn render_responses(out: &mut String, responses: &[ResponseIr]) -> Result<()> {
    if responses.is_empty() {
        out.push_str("    pub type Response = ();\n\n");
        return Ok(());
    }

    for response in responses {
        render_single_response(out, response)?;
    }

    out.push_str("    #[derive(Debug, Clone, Serialize, Deserialize)]\n");
    out.push_str("    pub enum Response {\n");
    for response in responses {
        let variant = response_variant_name(&response.status);
        let ty = response_type_name(&response.status);
        out.push_str(&format!("        {variant}({ty}),\n"));
    }
    out.push_str("    }\n\n");

    Ok(())
}

fn render_single_response(out: &mut String, response: &ResponseIr) -> Result<()> {
    let response_name = response_type_name(&response.status);
    let headers_name = format!("{response_name}Headers");
    let body_name = format!("{response_name}Body");

    render_response_headers(out, &headers_name, &response.headers)?;

    let body_ty = if response.contents.is_empty() {
        "()".to_string()
    } else if response.contents.len() == 1 {
        render_type_expr_in_module(&response.contents[0].schema, "crate::models::")?
    } else {
        out.push_str("    #[derive(Debug, Clone, Serialize, Deserialize)]\n");
        out.push_str(&format!("    pub enum {body_name} {{\n"));
        for media in &response.contents {
            let variant = rust_type_name(&media.media_type);
            let ty = render_type_expr_in_module(&media.schema, "crate::models::")?;
            out.push_str(&format!("        {variant}({ty}),\n"));
        }
        out.push_str("    }\n\n");
        body_name
    };

    out.push_str("    #[derive(Debug, Clone, Serialize, Deserialize)]\n");
    out.push_str(&format!("    pub struct {response_name} {{\n"));
    out.push_str(&format!("        pub body: {body_ty},\n"));
    out.push_str(&format!("        pub headers: {headers_name},\n"));
    out.push_str("    }\n\n");
    Ok(())
}

fn render_response_headers(out: &mut String, name: &str, headers: &[HeaderIr]) -> Result<()> {
    out.push_str("    #[derive(Debug, Clone, Default, Serialize, Deserialize)]\n");
    out.push_str(&format!("    pub struct {name} {{\n"));
    for header in headers {
        if let Some(description) = header.description.as_deref() {
            push_doc_comment(out, description, 8);
        }
        let field_name = rust_field_name(&header.name);
        let ty = render_type_expr_in_module(&header.ty, "crate::models::")?;
        let ty = if header.required {
            ty
        } else {
            format!("Option<{ty}>")
        };
        if field_name != header.name {
            out.push_str(&format!(
                "        #[serde(rename = \"{}\")]\n",
                escape_string(&header.name)
            ));
        }
        out.push_str(&format!("        pub {field_name}: {ty},\n"));
    }
    out.push_str("    }\n\n");
    Ok(())
}

fn render_client_rs(operations: &[Operation]) -> Result<String> {
    let mut out = String::from(
        "use crate::{operations, API_BASE_URL};\nuse std::fmt;\n\n#[derive(Debug)]\npub enum ClientError {\n    Transport(reqwest::Error),\n    UnexpectedStatus {\n        operation: &'static str,\n        status: u16,\n        body: String,\n    },\n}\n\nimpl fmt::Display for ClientError {\n    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {\n        match self {\n            Self::Transport(err) => write!(f, \"request failed: {err}\"),\n            Self::UnexpectedStatus { operation, status, .. } => {\n                write!(f, \"unexpected status {status} for operation {operation}\")\n            }\n        }\n    }\n}\n\nimpl std::error::Error for ClientError {\n    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {\n        match self {\n            Self::Transport(err) => Some(err),\n            Self::UnexpectedStatus { .. } => None,\n        }\n    }\n}\n\nimpl From<reqwest::Error> for ClientError {\n    fn from(value: reqwest::Error) -> Self {\n        Self::Transport(value)\n    }\n}\n\n#[derive(Debug, Clone)]\npub struct Client {\n    base_url: String,\n    http_client: reqwest::Client,\n}\n\nimpl Default for Client {\n    fn default() -> Self {\n        Self::new()\n    }\n}\n\nimpl Client {\n    pub fn new() -> Self {\n        Self {\n            base_url: API_BASE_URL.to_string(),\n            http_client: reqwest::Client::new(),\n        }\n    }\n\n    pub fn with_base_url(base_url: impl Into<String>) -> Self {\n        Self {\n            base_url: base_url.into(),\n            http_client: reqwest::Client::new(),\n        }\n    }\n\n    pub fn with_http_client(base_url: impl Into<String>, http_client: reqwest::Client) -> Self {\n        Self {\n            base_url: base_url.into(),\n            http_client,\n        }\n    }\n\n    pub fn base_url(&self) -> &str {\n        &self.base_url\n    }\n\n",
    );

    for operation in operations {
        render_client_method(&mut out, operation)?;
    }

    out.push_str("}\n");
    Ok(out)
}

fn render_client_method(out: &mut String, operation: &Operation) -> Result<()> {
    let module_name = rust_field_name(&operation.id);
    let method_name = rust_field_name(&operation.id);
    let build_method_name = format!("build_{method_name}_request");
    let send_method_name = format!("send_{method_name}");
    let path_params = operation
        .params
        .iter()
        .filter(|param| param.location == ParameterLocation::Path)
        .count()
        > 0;
    let query_params = operation
        .params
        .iter()
        .filter(|param| param.location == ParameterLocation::Query)
        .count()
        > 0;
    let header_params = operation
        .params
        .iter()
        .filter(|param| param.location == ParameterLocation::Header)
        .count()
        > 0;
    let cookie_params = operation
        .params
        .iter()
        .filter(|param| param.location == ParameterLocation::Cookie)
        .count()
        > 0;

    let mut args = Vec::new();
    let mut fields = Vec::new();

    if path_params {
        args.push("path_params: operations::".to_string() + &module_name + "::PathParams");
        fields.push("path_params".to_string());
    }
    if query_params {
        args.push("query_params: operations::".to_string() + &module_name + "::QueryParams");
        fields.push("query_params".to_string());
    }
    if header_params {
        args.push("header_params: operations::".to_string() + &module_name + "::HeaderParams");
        fields.push("header_params".to_string());
    }
    if cookie_params {
        args.push("cookie_params: operations::".to_string() + &module_name + "::CookieParams");
        fields.push("cookie_params".to_string());
    }
    if let Some(body) = operation.request_body.as_ref()
        && body.required
    {
        args.push("body: operations::".to_string() + &module_name + "::RequestBody");
        fields.push("body".to_string());
    }

    let signature_args = if args.is_empty() {
        "&self".to_string()
    } else {
        format!("&self, {}", args.join(", "))
    };

    out.push_str(&format!(
        "    pub fn {method_name}({signature_args}) -> operations::{module_name}::Operation {{\n"
    ));
    out.push_str(&format!(
        "        operations::{module_name}::Operation {{\n"
    ));
    for field in fields {
        out.push_str(&format!("            {field},\n"));
    }
    if operation
        .request_body
        .as_ref()
        .is_some_and(|body| !body.required)
    {
        out.push_str("            body: None,\n");
    }
    out.push_str("        }\n");
    out.push_str("    }\n\n");

    out.push_str(&format!(
        "    pub fn {build_method_name}(&self, operation: &operations::{module_name}::Operation) -> reqwest::RequestBuilder {{\n"
    ));
    out.push_str("        let url = format!(\"{}{}\", self.base_url, operation.path());\n");
    out.push_str(&format!(
        "        let mut request = self.http_client.request(reqwest::Method::{}, url);\n",
        reqwest_method_name(operation.method)
    ));
    if query_params {
        out.push_str("        request = request.query(&operation.query_params);\n");
    }
    if header_params {
        for param in operation
            .params
            .iter()
            .filter(|param| param.location == ParameterLocation::Header)
        {
            let field_name = rust_field_name(&param.name);
            if param.required {
                out.push_str(&format!(
                    "        request = request.header(\"{}\", operation.header_params.{}.to_string());\n",
                    escape_string(&param.name),
                    field_name
                ));
            } else {
                out.push_str(&format!(
                    "        if let Some(value) = &operation.header_params.{} {{\n            request = request.header(\"{}\", value.to_string());\n        }}\n",
                    field_name,
                    escape_string(&param.name)
                ));
            }
        }
    }
    if cookie_params {
        out.push_str("        let mut cookies = Vec::new();\n");
        for param in operation
            .params
            .iter()
            .filter(|param| param.location == ParameterLocation::Cookie)
        {
            let field_name = rust_field_name(&param.name);
            if param.required {
                out.push_str(&format!(
                    "        cookies.push(format!(\"{}={{}}\", operation.cookie_params.{}.to_string()));\n",
                    escape_string(&param.name),
                    field_name
                ));
            } else {
                out.push_str(&format!(
                    "        if let Some(value) = &operation.cookie_params.{} {{\n            cookies.push(format!(\"{}={{}}\", value.to_string()));\n        }}\n",
                    field_name,
                    escape_string(&param.name)
                ));
            }
        }
        out.push_str("        if !cookies.is_empty() {\n");
        out.push_str(
            "            request = request.header(reqwest::header::COOKIE, cookies.join(\"; \"));\n",
        );
        out.push_str("        }\n");
    }
    if let Some(body) = operation.request_body.as_ref() {
        if body.required {
            out.push_str("        request = request.json(&operation.body);\n");
        } else {
            out.push_str(
                "        if let Some(body) = &operation.body {\n            request = request.json(body);\n        }\n",
            );
        }
    }
    out.push_str("        request\n");
    out.push_str("    }\n\n");

    out.push_str(&format!(
        "    pub async fn {send_method_name}(&self, operation: &operations::{module_name}::Operation) -> Result<reqwest::Response, reqwest::Error> {{\n"
    ));
    out.push_str(&format!(
        "        self.{build_method_name}(operation).send().await\n"
    ));
    out.push_str("    }\n\n");

    if operation_supports_response_decoding(operation) {
        out.push_str(&format!(
            "    pub async fn execute_{method_name}(&self, operation: &operations::{module_name}::Operation) -> Result<operations::{module_name}::Response, ClientError> {{\n"
        ));
        out.push_str(&format!(
            "        let response = self.{send_method_name}(operation).await?;\n"
        ));
        out.push_str("        let status = response.status().as_u16();\n");
        out.push_str("        match status {\n");
        for response in operation
            .responses
            .iter()
            .filter(|response| !matches!(response.status, ResponseStatus::Default))
        {
            render_response_decode_branch(out, &module_name, response)?;
        }
        if let Some(default_response) = operation
            .responses
            .iter()
            .find(|response| matches!(response.status, ResponseStatus::Default))
        {
            out.push_str("            _ => {\n");
            render_response_decode_body(out, &module_name, default_response, 16)?;
            out.push_str("            }\n");
        } else {
            out.push_str("            _ => {\n");
            out.push_str("                let body = response.text().await.map_err(ClientError::Transport)?;\n");
            out.push_str(&format!(
                "                Err(ClientError::UnexpectedStatus {{ operation: operations::{module_name}::META.id, status, body }})\n"
            ));
            out.push_str("            }\n");
        }
        out.push_str("        }\n");
        out.push_str("    }\n\n");
    }

    Ok(())
}

fn operation_supports_response_decoding(operation: &Operation) -> bool {
    !operation.responses.is_empty() && operation.responses.iter().all(response_supports_decoding)
}

fn response_supports_decoding(response: &ResponseIr) -> bool {
    (response_status_code(&response.status).is_some()
        || matches!(response.status, ResponseStatus::Default))
        && response.headers.is_empty()
        && response_body_supports_decoding(&response.contents)
}

fn response_body_supports_decoding(contents: &[nokturn_core::ir::MediaTypeIr]) -> bool {
    contents.is_empty() || contents.len() == 1 && is_json_media_type(&contents[0].media_type)
}

fn response_status_code(status: &ResponseStatus) -> Option<u16> {
    match status {
        ResponseStatus::Code(code) => code.parse().ok(),
        ResponseStatus::Default => None,
    }
}

fn render_response_decode_branch(
    out: &mut String,
    module_name: &str,
    response: &ResponseIr,
) -> Result<()> {
    let status_code = response_status_code(&response.status)
        .ok_or_else(|| anyhow!("response decoding requires an exact numeric status code"))?;
    out.push_str(&format!("            {status_code} => {{\n"));
    render_response_decode_body(out, module_name, response, 16)?;
    out.push_str("            }\n");
    Ok(())
}

fn render_response_decode_body(
    out: &mut String,
    module_name: &str,
    response: &ResponseIr,
    indent: usize,
) -> Result<()> {
    let indent_prefix = " ".repeat(indent);
    let response_name = response_type_name(&response.status);
    let variant_name = response_variant_name(&response.status);

    if response.contents.is_empty() {
        out.push_str(&format!(
            "{indent_prefix}Ok(operations::{module_name}::Response::{variant_name}(operations::{module_name}::{response_name} {{ body: (), headers: Default::default() }}))\n"
        ));
        return Ok(());
    }

    let body_type = render_type_expr_in_module(&response.contents[0].schema, "crate::models::")?;
    out.push_str(&format!(
        "{indent_prefix}let body = response.json::<{body_type}>().await.map_err(ClientError::Transport)?;\n"
    ));
    out.push_str(&format!(
        "{indent_prefix}Ok(operations::{module_name}::Response::{variant_name}(operations::{module_name}::{response_name} {{ body, headers: Default::default() }}))\n"
    ));
    Ok(())
}

fn http_method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Put => "PUT",
        HttpMethod::Post => "POST",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Options => "OPTIONS",
        HttpMethod::Head => "HEAD",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Trace => "TRACE",
    }
}

fn reqwest_method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Put => "PUT",
        HttpMethod::Post => "POST",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Options => "OPTIONS",
        HttpMethod::Head => "HEAD",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Trace => "TRACE",
    }
}

fn response_variant_name(status: &ResponseStatus) -> String {
    match status {
        ResponseStatus::Code(code) => format!("Status{code}"),
        ResponseStatus::Default => "Default".to_string(),
    }
}

fn response_type_name(status: &ResponseStatus) -> String {
    match status {
        ResponseStatus::Code(code) => format!("Response{code}"),
        ResponseStatus::Default => "DefaultResponse".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::generate_sdk;
    use crate::Config;

    fn temp_output_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        env::temp_dir().join(format!("nokturn-{name}-{stamp}"))
    }

    #[tokio::test]
    async fn generates_model_crate_layout() {
        let spec = include_str!("../../nokturn-core/tests/fixtures/ir_schema.yaml");
        let config = Config {
            crate_name: "example-sdk".to_string(),
            version: "0.1.0".to_string(),
            edition: Some("2021".to_string()),
            description: "Example SDK".to_string(),
            lib_status: "experimental".to_string(),
            keywords: vec!["example".to_string()],
            api_url: "https://api.example.test".to_string(),
            authors: vec!["Example <example@test>".to_string()],
            include_only: None,
            exclude: None,
        };
        let output = temp_output_dir("models");

        generate_sdk(&config, spec, "yaml", &output)
            .await
            .expect("generate sdk");

        let cargo_toml = std::fs::read_to_string(output.join("Cargo.toml")).expect("Cargo.toml");
        let lib_rs = std::fs::read_to_string(output.join("src/lib.rs")).expect("lib.rs");
        let models_rs = std::fs::read_to_string(output.join("src/models.rs")).expect("models.rs");
        let operations_rs =
            std::fs::read_to_string(output.join("src/operations.rs")).expect("operations.rs");
        let client_rs = std::fs::read_to_string(output.join("src/client.rs")).expect("client.rs");

        assert!(cargo_toml.contains("name = \"example-sdk\""));
        assert!(cargo_toml.contains("[dependencies.reqwest]"));
        assert!(cargo_toml.contains("version = \"0.13\""));
        assert!(cargo_toml.contains("features = [\"json\"]"));
        assert!(lib_rs.contains("pub mod models;"));
        assert!(lib_rs.contains("pub mod operations;"));
        assert!(lib_rs.contains("pub mod client;"));
        assert!(lib_rs.contains("pub use client::{Client, ClientError};"));
        assert!(lib_rs.contains("API_BASE_URL"));
        assert!(models_rs.contains("pub struct Item"));
        assert!(models_rs.contains("pub id: String"));
        assert!(operations_rs.contains("pub mod create_item"));
        assert!(operations_rs.contains("pub struct Operation"));
        assert!(operations_rs.contains("pub type RequestBody = crate::models::NewItem;"));
        assert!(operations_rs.contains("pub struct Response201"));
        assert!(operations_rs.contains("pub fn path(&self) -> String"));
        assert!(client_rs.contains("pub struct Client"));
        assert!(client_rs.contains("pub enum ClientError"));
        assert!(client_rs.contains("pub fn create_item"));
        assert!(client_rs.contains("pub fn build_create_item_request"));
        assert!(client_rs.contains("pub async fn send_create_item"));
        assert!(client_rs.contains("pub async fn execute_create_item"));
        assert!(client_rs.contains("self.http_client.request(reqwest::Method::POST, url)"));
        assert!(client_rs.contains("self.build_create_item_request(operation).send().await"));
        assert!(client_rs.contains("let response = self.send_create_item(operation).await?;"));
        assert!(client_rs.contains("response.json::<crate::models::Item>()"));
        assert!(client_rs.contains("request = request.json(&operation.body);"));

        let _ = std::fs::remove_dir_all(output);
    }

    #[tokio::test]
    async fn generation_fails_on_lowering_diagnostics() {
        let spec = r#"
openapi: 3.0.0
info:
  title: Broken
  version: "1.0"
paths:
  /items:
    get:
      operationId: listItems
      responses:
        '200':
          $ref: '#/paths/~1items/get'
"#;
        let config = Config {
            crate_name: "broken-sdk".to_string(),
            version: "0.1.0".to_string(),
            edition: None,
            description: "Broken SDK".to_string(),
            lib_status: "experimental".to_string(),
            keywords: vec!["broken".to_string()],
            api_url: "https://api.example.test".to_string(),
            authors: vec!["Example <example@test>".to_string()],
            include_only: None,
            exclude: None,
        };
        let output = temp_output_dir("diagnostics");

        let error = generate_sdk(&config, spec, "yaml", &output)
            .await
            .expect_err("generation should fail");

        assert!(error.to_string().contains("lowering emitted diagnostics"));
        let _ = std::fs::remove_dir_all(output);
    }

    #[tokio::test]
    async fn generation_fails_on_unsupported_request_body_media_types() {
        let spec = r#"
openapi: 3.0.0
info:
  title: Unsupported Request Bodies
  version: "1.0"
paths:
  /items:
    post:
      operationId: createItem
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                id:
                  type: string
          text/plain:
            schema:
              type: string
      responses:
        '201':
          description: Created
"#;
        let config = Config {
            crate_name: "broken-sdk".to_string(),
            version: "0.1.0".to_string(),
            edition: None,
            description: "Broken SDK".to_string(),
            lib_status: "experimental".to_string(),
            keywords: vec!["broken".to_string()],
            api_url: "https://api.example.test".to_string(),
            authors: vec!["Example <example@test>".to_string()],
            include_only: None,
            exclude: None,
        };
        let output = temp_output_dir("unsupported-request-body");

        let error = generate_sdk(&config, spec, "yaml", &output)
            .await
            .expect_err("generation should fail");

        assert!(
            error
                .to_string()
                .contains("multiple request body media types")
        );
        let _ = std::fs::remove_dir_all(output);
    }

    #[tokio::test]
    async fn generation_skips_typed_execute_for_unsupported_response_media_types() {
        let spec = r#"
openapi: 3.0.0
info:
  title: Plain Text Responses
  version: "1.0"
paths:
  /items:
    get:
      operationId: listItems
      responses:
        '200':
          description: Ok
          content:
            text/plain:
              schema:
                type: string
"#;
        let config = Config {
            crate_name: "plain-sdk".to_string(),
            version: "0.1.0".to_string(),
            edition: Some("2021".to_string()),
            description: "Plain SDK".to_string(),
            lib_status: "experimental".to_string(),
            keywords: vec!["plain".to_string()],
            api_url: "https://api.example.test".to_string(),
            authors: vec!["Example <example@test>".to_string()],
            include_only: None,
            exclude: None,
        };
        let output = temp_output_dir("plain-response");

        generate_sdk(&config, spec, "yaml", &output)
            .await
            .expect("generate sdk");

        let client_rs = std::fs::read_to_string(output.join("src/client.rs")).expect("client.rs");

        assert!(client_rs.contains("pub async fn send_list_items"));
        assert!(!client_rs.contains("pub async fn execute_list_items"));

        let _ = std::fs::remove_dir_all(output);
    }
}
