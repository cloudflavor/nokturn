use nokturn_core::{
    ir::{ModelId, ModelKind, ResponseStatus, build_api_ir},
    load_openapiv3,
    lowering::{LoweringDiagnosticKind, LoweringDiagnosticTarget, build_lowered_api},
    resolver::EagerResolver,
    resolver::ResolveResult,
};
use openapiv3::ReferenceOr;

#[test]
fn resolves_simple_spec() {
    let spec_str = include_str!("fixtures/simple.yaml");
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let mut operations = Vec::new();
    resolved.for_each_operation(|entry| {
        operations.push((entry.path.to_string(), entry.method));
    });

    assert_eq!(operations.len(), 1);

    let ir = build_api_ir(&resolved);
    assert_eq!(ir.operations.len(), 1);
    let op = &ir.operations[0];
    assert_eq!(op.id, "listItems");
    assert_eq!(op.path, "/items");
    assert_eq!(op.params.len(), 1);
    assert!(
        op.responses
            .iter()
            .any(|resp| matches!(resp.status, ResponseStatus::Code(ref code) if code == "200"))
    );
}

#[test]
fn ir_builds_models_and_types() {
    let spec_str = include_str!("fixtures/ir_schema.yaml");
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");
    let ir = build_api_ir(&resolved);

    assert_eq!(ir.operations.len(), 1);
    let op = &ir.operations[0];
    assert_eq!(op.id, "createItem");
    assert!(op.request_body.is_some());
    assert!(
        op.responses
            .iter()
            .any(|resp| matches!(resp.status, ResponseStatus::Code(ref code) if code == "201"))
    );

    assert!(ir.models.contains_key(&ModelId("Item".to_string())));
    let item = ir.models.get(&ModelId("Item".to_string())).unwrap();
    match &item.kind {
        ModelKind::Struct { fields, .. } => {
            assert!(fields.iter().any(|f| f.name == "id"));
            assert!(fields.iter().any(|f| f.name == "tags"));
        }
        _ => panic!("expected struct model"),
    }
}

#[test]
fn resolves_referenced_path_level_parameters() {
    let spec_str = r#"
openapi: 3.0.0
info:
  title: Path Parameters API
  version: "1.0"
paths:
  /items/{item_id}:
    parameters:
      - $ref: '#/components/parameters/ItemId'
    get:
      operationId: getItem
      responses:
        '200':
          description: OK
components:
  parameters:
    ItemId:
      name: item_id
      in: path
      required: true
      schema:
        type: string
"#;
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let ir = build_api_ir(&resolved);
    let op = &ir.operations[0];

    assert_eq!(op.id, "getItem");
    assert_eq!(op.params.len(), 1);
    assert_eq!(op.params[0].name, "item_id");
}

#[test]
fn operation_parameters_override_path_parameters() {
    let spec_str = r#"
openapi: 3.0.0
info:
  title: Parameter Override API
  version: "1.0"
paths:
  /items:
    parameters:
      - name: page
        in: query
        required: false
        description: path-level parameter
        schema:
          type: integer
    get:
      operationId: listItems
      parameters:
        - name: page
          in: query
          required: true
          description: operation-level parameter
          schema:
            type: integer
      responses:
        '200':
          description: OK
"#;
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let ir = build_api_ir(&resolved);
    let op = &ir.operations[0];

    assert_eq!(op.params.len(), 1);
    assert_eq!(op.params[0].name, "page");
    assert!(op.params[0].required);
    assert_eq!(
        op.params[0].description.as_deref(),
        Some("operation-level parameter")
    );
}

#[test]
fn eager_resolver_indexes_unreferenced_component_schemas() {
    let spec_str = r#"
openapi: 3.0.0
info:
  title: Unreferenced Schema API
  version: "1.0"
paths:
  /items:
    get:
      operationId: listItems
      responses:
        '200':
          description: OK
components:
  schemas:
    Item:
      type: object
      properties:
        id:
          type: string
    Unused:
      type: object
      properties:
        name:
          type: string
"#;
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let ir = build_api_ir(&resolved);

    assert!(ir.models.contains_key(&ModelId("Item".to_string())));
    assert!(ir.models.contains_key(&ModelId("Unused".to_string())));
}

#[test]
fn resolves_top_level_path_item_references() {
    let spec_str = r#"
openapi: 3.0.0
info:
  title: Path Item Reference API
  version: "1.0"
paths:
  /aliased:
    $ref: '#/paths/~1shared'
  /shared:
    parameters:
      - $ref: '#/components/parameters/SharedHeader'
    get:
      operationId: getShared
      responses:
        '200':
          description: OK
components:
  parameters:
    SharedHeader:
      name: x-request-id
      in: header
      required: false
      schema:
        type: string
"#;
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let ir = build_api_ir(&resolved);

    assert_eq!(ir.operations.len(), 2);

    let aliased = ir
        .operations
        .iter()
        .find(|operation| operation.path == "/aliased")
        .expect("aliased operation");
    assert_eq!(aliased.id, "getShared");
    assert_eq!(aliased.params.len(), 1);
    assert_eq!(aliased.params[0].name, "x-request-id");
}

#[test]
fn resolves_chained_top_level_path_item_references() {
    let spec_str = r#"
openapi: 3.0.0
info:
  title: Chained Path Item Reference API
  version: "1.0"
paths:
  /first:
    $ref: '#/paths/~1second'
  /second:
    $ref: '#/paths/~1shared'
  /shared:
    get:
      operationId: getShared
      responses:
        '200':
          description: OK
"#;
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let ir = build_api_ir(&resolved);

    assert_eq!(ir.operations.len(), 3);
    assert!(
        ir.operations
            .iter()
            .any(|operation| operation.path == "/first")
    );
    assert!(
        ir.operations
            .iter()
            .any(|operation| operation.path == "/second")
    );
    assert!(
        ir.operations
            .iter()
            .any(|operation| operation.path == "/shared")
    );
}

#[test]
fn inline_and_component_responses_do_not_collide() {
    let spec_str = r#"
openapi: 3.0.0
info:
  title: Response Collision API
  version: "1.0"
paths:
  /inline:
    get:
      operationId: getInline
      responses:
        '200':
          description: inline response
  /component:
    get:
      operationId: getComponent
      responses:
        '200':
          $ref: '#/components/responses/200'
components:
  responses:
    '200':
      description: component response
"#;
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let ir = build_api_ir(&resolved);

    let inline = ir
        .operations
        .iter()
        .find(|operation| operation.path == "/inline")
        .expect("inline operation");
    let inline_response = inline
        .responses
        .iter()
        .find(|response| matches!(response.status, ResponseStatus::Code(ref code) if code == "200"))
        .expect("inline response");
    assert_eq!(
        inline_response.description.as_deref(),
        Some("inline response")
    );

    let component = ir
        .operations
        .iter()
        .find(|operation| operation.path == "/component")
        .expect("component operation");
    let component_response = component
        .responses
        .iter()
        .find(|response| matches!(response.status, ResponseStatus::Code(ref code) if code == "200"))
        .expect("component response");
    assert_eq!(
        component_response.description.as_deref(),
        Some("component response")
    );
}

#[test]
fn inline_and_component_headers_do_not_collide() {
    let spec_str = r#"
openapi: 3.0.0
info:
  title: Header Collision API
  version: "1.0"
paths:
  /inline:
    get:
      operationId: getInline
      responses:
        '200':
          description: inline response
          headers:
            X-Request-Id:
              description: inline header
              required: true
              schema:
                type: string
  /component:
    get:
      operationId: getComponent
      responses:
        '200':
          description: component response
          headers:
            X-Request-Id:
              $ref: '#/components/headers/X-Request-Id'
components:
  headers:
    X-Request-Id:
      description: component header
      required: false
      schema:
        type: integer
"#;
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let ir = build_api_ir(&resolved);

    let inline = ir
        .operations
        .iter()
        .find(|operation| operation.path == "/inline")
        .expect("inline operation");
    let inline_header = inline.responses[0]
        .headers
        .iter()
        .find(|header| header.name == "X-Request-Id")
        .expect("inline header");
    assert_eq!(inline_header.description.as_deref(), Some("inline header"));
    assert!(inline_header.required);

    let component = ir
        .operations
        .iter()
        .find(|operation| operation.path == "/component")
        .expect("component operation");
    let component_header = component.responses[0]
        .headers
        .iter()
        .find(|header| header.name == "X-Request-Id")
        .expect("component header");
    assert_eq!(
        component_header.description.as_deref(),
        Some("component header")
    );
    assert!(!component_header.required);
}

#[test]
fn resolve_result_reports_missing_component_reference() {
    let spec_str = include_str!("fixtures/simple.yaml");
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let response = ReferenceOr::Reference {
        reference: "#/components/responses/DoesNotExist".to_string(),
    };

    assert_eq!(
        resolved.resolve_response_result(&response),
        ResolveResult::MissingReference {
            reference: "#/components/responses/DoesNotExist",
        }
    );
}

#[test]
fn resolve_result_reports_unsupported_reference_kind() {
    let spec_str = include_str!("fixtures/simple.yaml");
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let response = ReferenceOr::Reference {
        reference: "#/paths/~1items/get".to_string(),
    };

    assert_eq!(
        resolved.resolve_response_result(&response),
        ResolveResult::UnsupportedReference {
            reference: "#/paths/~1items/get",
        }
    );
}

#[test]
fn lowering_reports_unsupported_response_reference_diagnostics() {
    let spec_str = r#"
openapi: 3.0.0
info:
  title: Unsupported Response Diagnostic API
  version: "1.0"
paths:
  /items:
    get:
      operationId: listItems
      responses:
        '200':
          $ref: '#/paths/~1items/get'
"#;
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let lowered = build_lowered_api(&resolved);

    assert_eq!(lowered.operations.len(), 1);
    assert!(lowered.operations[0].responses.is_empty());
    assert_eq!(lowered.diagnostics.len(), 1);
    assert_eq!(
        lowered.diagnostics[0].kind,
        LoweringDiagnosticKind::UnsupportedReference
    );
    assert_eq!(
        lowered.diagnostics[0].target,
        LoweringDiagnosticTarget::Response
    );
    assert_eq!(lowered.diagnostics[0].reference, "#/paths/~1items/get");
}

#[test]
fn lowering_reports_unsupported_schema_reference_diagnostics() {
    let spec_str = r#"
openapi: 3.0.0
info:
  title: Unsupported Schema Diagnostic API
  version: "1.0"
paths:
  /items:
    get:
      operationId: listItems
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                $ref: '#/paths/~1items/get'
"#;
    let schema = load_openapiv3(spec_str, "yaml").expect("load spec");
    let resolver = EagerResolver::new(&schema);
    let resolved = resolver.resolve().expect("resolve spec");

    let lowered = build_lowered_api(&resolved);

    assert!(lowered.diagnostics.iter().any(|diagnostic| diagnostic.kind
        == LoweringDiagnosticKind::UnsupportedReference
        && diagnostic.target == LoweringDiagnosticTarget::Schema
        && diagnostic.reference == "#/paths/~1items/get"));
}
