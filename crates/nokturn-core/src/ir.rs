use std::collections::BTreeMap;

use crate::{lowering, resolver::ResolvedSpec, utils::HttpMethod};

pub fn build_api_ir(resolved: &ResolvedSpec<'_>) -> ApiIr {
    lowering::build_api_ir(resolved)
}

/// Canonical IR extracted from an OpenAPI 3.0 document for downstream codegen.
#[derive(Debug, Clone)]
pub struct ApiIr {
    pub info: SpecInfo,
    pub models: BTreeMap<ModelId, Model>,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, Default)]
pub struct SpecInfo {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelId(pub String);

#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    pub docs: Option<String>,
    pub deprecated: bool,
    pub kind: ModelKind,
}

#[derive(Debug, Clone)]
pub enum ModelKind {
    Struct {
        fields: Vec<Field>,
        additional: AdditionalBehavior,
    },
    Enum {
        repr: EnumRepr,
        variants: Vec<EnumVariant>,
    },
    Union {
        discriminator: Option<Discriminator>,
        variants: Vec<TypeExpr>,
    },
    Alias(TypeExpr),
}

#[derive(Debug, Clone)]
pub struct Discriminator {
    pub property_name: String,
    pub mapping: BTreeMap<String, ModelId>,
}

#[derive(Debug, Clone)]
pub enum AdditionalBehavior {
    Any,                  // additionalProperties: true or omitted
    Forbidden,            // additionalProperties: false
    Typed(Box<TypeExpr>), // schema object
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub json_name: String,
    pub ty: TypeExpr,
    pub required: bool,
    pub metadata: FieldMetadata,
}

#[derive(Debug, Clone, Default)]
pub struct FieldMetadata {
    pub description: Option<String>,
    pub deprecated: bool,
    pub read_only: bool,
    pub write_only: bool,
    pub validations: ValidationRules,
    pub default_value: Option<LiteralValue>,
    pub example: Option<LiteralValue>,
}

#[derive(Debug, Clone)]
pub enum TypeExpr {
    Primitive(PrimitiveKind),
    Literal(LiteralValue),
    Array(Box<TypeExpr>),
    Map {
        value: Box<TypeExpr>,
        additional: AdditionalBehavior,
    },
    Optional(Box<TypeExpr>), // required = false
    Nullable(Box<TypeExpr>), // nullable = true
    Reference(ModelId),
    OneOf(Vec<TypeExpr>),
    AnyOf(Vec<TypeExpr>),
    AllOf(Vec<TypeExpr>),
    DiscriminatedUnion {
        property: String,
        mapping: BTreeMap<String, TypeExpr>,
    },
}

#[derive(Debug, Clone)]
pub enum PrimitiveKind {
    Boolean,
    Integer32,
    Integer64,
    NumberFloat,
    NumberDouble,
    String,
    StringByte,
    StringBinary,
    StringDate,
    StringDateTime,
    StringPassword,
    StringUuid,
}

#[derive(Debug, Clone)]
pub enum LiteralValue {
    Integer(i64),
    Number(f64),
    Boolean(bool),
    String(String),
}

#[derive(Debug, Clone, Default)]
pub struct ValidationRules {
    pub min: Option<NumberBound>,
    pub max: Option<NumberBound>,
    pub multiple_of: Option<f64>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub pattern: Option<String>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub unique_items: bool,
    pub min_properties: Option<u64>,
    pub max_properties: Option<u64>,
    pub enumeration: Vec<LiteralValue>,
}

#[derive(Debug, Clone)]
pub struct NumberBound {
    pub value: f64,
    pub inclusive: bool,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    pub value: LiteralValue,
    pub docs: Option<String>,
}

#[derive(Debug, Clone)]
pub enum EnumRepr {
    String,
    Integer,
    Number,
}

#[derive(Debug, Clone)]
pub struct Operation {
    pub id: String,
    pub path: String,
    pub method: HttpMethod,
    pub tags: Vec<String>,
    pub summary: Option<String>,
    pub docs: Option<String>,
    pub deprecated: bool,
    pub params: Vec<ParameterIr>,
    pub request_body: Option<RequestBodyIr>,
    pub responses: Vec<ResponseIr>,
}

#[derive(Debug, Clone)]
pub struct ParameterIr {
    pub name: String,
    pub location: ParameterLocation,
    pub required: bool,
    pub deprecated: bool,
    pub description: Option<String>,
    pub ty: TypeExpr,
    pub style: ParameterStyle,
    pub explode: bool,
    pub allow_reserved: bool,
    pub example: Option<LiteralValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParameterLocation {
    Path,
    Query,
    Header,
    Cookie,
}

#[derive(Debug, Clone, Copy)]
pub enum ParameterStyle {
    Matrix,
    Label,
    Form,
    Simple,
    SpaceDelimited,
    PipeDelimited,
    DeepObject,
}

#[derive(Debug, Clone)]
pub struct RequestBodyIr {
    pub description: Option<String>,
    pub required: bool,
    pub contents: Vec<MediaTypeIr>,
}

#[derive(Debug, Clone)]
pub struct MediaTypeIr {
    pub media_type: String,
    pub schema: TypeExpr,
    pub encoding: Option<EncodingInfo>,
}

#[derive(Debug, Clone)]
pub struct EncodingInfo {
    pub style: Option<ParameterStyle>,
    pub explode: Option<bool>,
    pub allow_reserved: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ResponseIr {
    pub status: ResponseStatus,
    pub description: Option<String>,
    pub contents: Vec<MediaTypeIr>,
    pub headers: Vec<HeaderIr>,
}

#[derive(Debug, Clone)]
pub enum ResponseStatus {
    Code(String),
    Default,
}

#[derive(Debug, Clone)]
pub struct HeaderIr {
    pub name: String,
    pub description: Option<String>,
    pub ty: TypeExpr,
    pub required: bool,
    pub deprecated: bool,
}
