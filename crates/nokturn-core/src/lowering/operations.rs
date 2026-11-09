use indexmap::IndexMap;
use openapiv3::{
    CookieStyle, Encoding, Header as OAHeader, HeaderStyle, MediaType as OAMediaType,
    Operation as OAPIOperation, Parameter as OAParameter, PathStyle, QueryStyle, ReferenceOr,
    Response as OAResponse, Responses,
};

use crate::ir::{
    EncodingInfo, HeaderIr, LiteralValue, MediaTypeIr, Operation, ParameterIr,
    ParameterLocation, ParameterStyle, RequestBodyIr, ResponseIr, ResponseStatus, TypeExpr,
};
use crate::resolver::OperationEntry;
use crate::utils::HttpMethod;

use super::{LoweringDiagnosticTarget, Resolver, TypeLowerer};

impl<R: Resolver> TypeLowerer<'_, R> {
    pub(super) fn build_operations(&self) -> Vec<Operation> {
        self.resolver
            .operations()
            .map(|entry| self.lower_operation(entry))
            .collect()
    }

    pub fn lower_operation(&self, entry: OperationEntry<'_>) -> Operation {
        let operation = entry.operation;
        Operation {
            id: operation_id(operation, entry.path, entry.method),
            path: entry.path.to_string(),
            method: entry.method,
            tags: operation.tags.clone(),
            summary: operation.summary.clone(),
            docs: operation.description.clone(),
            deprecated: operation.deprecated,
            params: self.build_parameters(&entry),
            request_body: self.build_request_body(operation),
            responses: self.build_responses(&operation.responses),
        }
    }

    fn build_parameters(&self, entry: &OperationEntry<'_>) -> Vec<ParameterIr> {
        let mut params = IndexMap::new();

        for param in &entry.path_item.parameters {
            self.insert_parameter(&mut params, param);
        }

        for param in &entry.operation.parameters {
            self.insert_parameter(&mut params, param);
        }

        params.into_values().collect()
    }

    fn insert_parameter(
        &self,
        params: &mut IndexMap<(String, ParameterLocation), ParameterIr>,
        param: &ReferenceOr<OAParameter>,
    ) {
        if let Some(parameter) = self.resolve_or_diagnostic(
            LoweringDiagnosticTarget::Parameter,
            self.resolver.resolve_parameter(param),
        ) {
            params.insert(
                parameter_identity(parameter),
                self.parameter_to_ir(parameter),
            );
        }
    }

    fn parameter_to_ir(&self, parameter: &OAParameter) -> ParameterIr {
        match parameter {
            OAParameter::Query {
                parameter_data,
                allow_reserved,
                style,
                allow_empty_value: _,
            } => ParameterIr {
                name: parameter_data.name.clone(),
                location: ParameterLocation::Query,
                required: parameter_data.required,
                deprecated: parameter_data.deprecated.unwrap_or(false),
                description: parameter_data.description.clone(),
                ty: self.schema_or_content_type(&parameter_data.format),
                style: map_query_style(style.clone()),
                explode: parameter_data.explode.unwrap_or(true),
                allow_reserved: *allow_reserved,
                example: parameter_data.example.as_ref().and_then(value_to_literal),
            },
            OAParameter::Header {
                parameter_data,
                style,
            } => ParameterIr {
                name: parameter_data.name.clone(),
                location: ParameterLocation::Header,
                required: parameter_data.required,
                deprecated: parameter_data.deprecated.unwrap_or(false),
                description: parameter_data.description.clone(),
                ty: self.schema_or_content_type(&parameter_data.format),
                style: map_header_style(style.clone()),
                explode: parameter_data.explode.unwrap_or(false),
                allow_reserved: false,
                example: parameter_data.example.as_ref().and_then(value_to_literal),
            },
            OAParameter::Path {
                parameter_data,
                style,
            } => ParameterIr {
                name: parameter_data.name.clone(),
                location: ParameterLocation::Path,
                required: true,
                deprecated: parameter_data.deprecated.unwrap_or(false),
                description: parameter_data.description.clone(),
                ty: self.schema_or_content_type(&parameter_data.format),
                style: map_path_style(style.clone()),
                explode: parameter_data.explode.unwrap_or(false),
                allow_reserved: false,
                example: parameter_data.example.as_ref().and_then(value_to_literal),
            },
            OAParameter::Cookie {
                parameter_data,
                style,
            } => ParameterIr {
                name: parameter_data.name.clone(),
                location: ParameterLocation::Cookie,
                required: parameter_data.required,
                deprecated: parameter_data.deprecated.unwrap_or(false),
                description: parameter_data.description.clone(),
                ty: self.schema_or_content_type(&parameter_data.format),
                style: map_cookie_style(style.clone()),
                explode: parameter_data.explode.unwrap_or(false),
                allow_reserved: false,
                example: parameter_data.example.as_ref().and_then(value_to_literal),
            },
        }
    }

    fn build_request_body(&self, operation: &OAPIOperation) -> Option<RequestBodyIr> {
        let body = operation.request_body.as_ref().and_then(|body| {
            self.resolve_or_diagnostic(
                LoweringDiagnosticTarget::RequestBody,
                self.resolver.resolve_request_body(body),
            )
        })?;

        Some(RequestBodyIr {
            description: body.description.clone(),
            required: body.required,
            contents: self.build_media_types(&body.content),
        })
    }

    fn build_responses(&self, responses: &Responses) -> Vec<ResponseIr> {
        let mut ir = Vec::new();
        if let Some(default) = responses.default.as_ref().and_then(|resp| {
            self.resolve_or_diagnostic(
                LoweringDiagnosticTarget::Response,
                self.resolver.resolve_response(resp),
            )
        }) {
            ir.push(self.response_to_ir(ResponseStatus::Default, default));
        }

        for (status, response) in responses.responses.iter() {
            if let Some(resp) = self.resolve_or_diagnostic(
                LoweringDiagnosticTarget::Response,
                self.resolver.resolve_response(response),
            ) {
                ir.push(self.response_to_ir(ResponseStatus::Code(status.to_string()), resp));
            }
        }

        ir
    }

    fn response_to_ir(&self, status: ResponseStatus, response: &OAResponse) -> ResponseIr {
        ResponseIr {
            status,
            description: Some(response.description.clone()),
            contents: self.build_media_types(&response.content),
            headers: self.build_headers(&response.headers),
        }
    }

    fn build_headers(&self, headers: &IndexMap<String, ReferenceOr<OAHeader>>) -> Vec<HeaderIr> {
        let mut result = Vec::new();
        for (name, header) in headers {
            if let Some(h) = self.resolve_or_diagnostic(
                LoweringDiagnosticTarget::Header,
                self.resolver.resolve_header(header),
            ) {
                result.push(HeaderIr {
                    name: name.clone(),
                    description: h.description.clone(),
                    ty: self.schema_or_content_type(&h.format),
                    required: h.required,
                    deprecated: h.deprecated.unwrap_or(false),
                });
            }
        }
        result
    }

    fn build_media_types(&self, content: &IndexMap<String, OAMediaType>) -> Vec<MediaTypeIr> {
        let mut result = Vec::new();
        for (media, media_type) in content {
            let schema = media_type
                .schema
                .as_ref()
                .map(|schema| self.lower_schema_ref(schema))
                .unwrap_or_else(|| TypeExpr::Primitive(crate::ir::PrimitiveKind::String));

            result.push(MediaTypeIr {
                media_type: media.clone(),
                schema,
                encoding: media_type.encoding.values().next().map(encoding_to_ir),
            });
        }
        result
    }
}

fn operation_id(operation: &OAPIOperation, path: &str, method: HttpMethod) -> String {
    if let Some(id) = operation.operation_id.as_ref() {
        return id.clone();
    }

    let normalized_path = path
        .trim_start_matches('/')
        .replace('/', "_")
        .replace(['{', '}'], "");

    let slug = if normalized_path.is_empty() {
        "root".to_string()
    } else {
        normalized_path
    };

    format!("{}_{}", method_name(method), slug)
}

fn method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "get",
        HttpMethod::Put => "put",
        HttpMethod::Post => "post",
        HttpMethod::Delete => "delete",
        HttpMethod::Options => "options",
        HttpMethod::Head => "head",
        HttpMethod::Patch => "patch",
        HttpMethod::Trace => "trace",
    }
}

fn parameter_identity(parameter: &OAParameter) -> (String, ParameterLocation) {
    (
        parameter.parameter_data_ref().name.clone(),
        parameter_location(parameter),
    )
}

fn parameter_location(parameter: &OAParameter) -> ParameterLocation {
    match parameter {
        OAParameter::Query { .. } => ParameterLocation::Query,
        OAParameter::Header { .. } => ParameterLocation::Header,
        OAParameter::Path { .. } => ParameterLocation::Path,
        OAParameter::Cookie { .. } => ParameterLocation::Cookie,
    }
}

fn encoding_to_ir(encoding: &Encoding) -> EncodingInfo {
    EncodingInfo {
        style: encoding.style.clone().map(map_query_style),
        explode: Some(encoding.explode),
        allow_reserved: Some(encoding.allow_reserved),
    }
}

fn map_query_style(style: QueryStyle) -> ParameterStyle {
    match style {
        QueryStyle::Form => ParameterStyle::Form,
        QueryStyle::SpaceDelimited => ParameterStyle::SpaceDelimited,
        QueryStyle::PipeDelimited => ParameterStyle::PipeDelimited,
        QueryStyle::DeepObject => ParameterStyle::DeepObject,
    }
}

fn map_header_style(style: HeaderStyle) -> ParameterStyle {
    match style {
        HeaderStyle::Simple => ParameterStyle::Simple,
    }
}

fn map_path_style(style: PathStyle) -> ParameterStyle {
    match style {
        PathStyle::Matrix => ParameterStyle::Matrix,
        PathStyle::Label => ParameterStyle::Label,
        PathStyle::Simple => ParameterStyle::Simple,
    }
}

fn map_cookie_style(style: CookieStyle) -> ParameterStyle {
    match style {
        CookieStyle::Form => ParameterStyle::Form,
    }
}

fn value_to_literal(value: &serde_json::Value) -> Option<LiteralValue> {
    match value {
        serde_json::Value::String(s) => Some(LiteralValue::String(s.clone())),
        serde_json::Value::Bool(b) => Some(LiteralValue::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(LiteralValue::Integer(i))
            } else {
                n.as_f64().map(LiteralValue::Number)
            }
        }
        _ => None,
    }
}
