use openapiv3::{
    AdditionalProperties as OAAdditionalProperties, IntegerFormat, NumberFormat,
    ParameterSchemaOrContent, ReferenceOr, Schema as OASchema, SchemaKind as OASchemaKind,
    StringFormat, Type as OAType, VariantOrUnknownOrEmpty,
};

use crate::ir::{AdditionalBehavior, ModelId, PrimitiveKind, TypeExpr};

use super::{LoweringDiagnosticTarget, Resolver, TypeLowerer};

impl<R: Resolver> TypeLowerer<'_, R> {
    pub fn lower_schema_ref(&self, schema: &ReferenceOr<OASchema>) -> TypeExpr {
        match schema {
            ReferenceOr::Item(item) => self.lower_schema_type(item),
            ReferenceOr::Reference { reference } => {
                let _ = self.resolve_or_diagnostic(
                    LoweringDiagnosticTarget::Schema,
                    self.resolver.resolve_schema(schema),
                );
                reference_to_model(reference)
            }
        }
    }

    pub(super) fn lower_boxed_schema_ref(&self, schema: &ReferenceOr<Box<OASchema>>) -> TypeExpr {
        match schema {
            ReferenceOr::Item(item) => self.lower_schema_type(item),
            ReferenceOr::Reference { reference } => reference_to_model(reference),
        }
    }

    pub(super) fn lower_schema_type(&self, schema: &OASchema) -> TypeExpr {
        let base = match &schema.schema_kind {
            OASchemaKind::Type(ty) => self.type_from_simple(ty),
            OASchemaKind::OneOf { one_of } => TypeExpr::OneOf(
                one_of
                    .iter()
                    .map(|schema| self.lower_schema_ref(schema))
                    .collect(),
            ),
            OASchemaKind::AnyOf { any_of } => TypeExpr::AnyOf(
                any_of
                    .iter()
                    .map(|schema| self.lower_schema_ref(schema))
                    .collect(),
            ),
            OASchemaKind::AllOf { all_of } => TypeExpr::AllOf(
                all_of
                    .iter()
                    .map(|schema| self.lower_schema_ref(schema))
                    .collect(),
            ),
            OASchemaKind::Not { .. } => TypeExpr::Primitive(PrimitiveKind::String),
            OASchemaKind::Any(_) => TypeExpr::Map {
                value: Box::new(TypeExpr::Primitive(PrimitiveKind::String)),
                additional: AdditionalBehavior::Any,
            },
        };

        if schema.schema_data.nullable {
            TypeExpr::Nullable(Box::new(base))
        } else {
            base
        }
    }

    pub(super) fn schema_or_content_type(&self, format: &ParameterSchemaOrContent) -> TypeExpr {
        match format {
            ParameterSchemaOrContent::Schema(schema) => self.lower_schema_ref(schema),
            ParameterSchemaOrContent::Content(content) => content
                .values()
                .next()
                .and_then(|media_type| media_type.schema.as_ref())
                .map(|schema| self.lower_schema_ref(schema))
                .unwrap_or_else(|| TypeExpr::Primitive(PrimitiveKind::String)),
        }
    }

    pub(super) fn additional_behavior(
        &self,
        additional: &Option<OAAdditionalProperties>,
    ) -> AdditionalBehavior {
        match additional {
            Some(OAAdditionalProperties::Any(true)) | None => AdditionalBehavior::Any,
            Some(OAAdditionalProperties::Any(false)) => AdditionalBehavior::Forbidden,
            Some(OAAdditionalProperties::Schema(schema)) => {
                AdditionalBehavior::Typed(Box::new(self.lower_schema_ref(schema.as_ref())))
            }
        }
    }

    fn type_from_simple(&self, ty: &OAType) -> TypeExpr {
        match ty {
            OAType::Boolean(_b) => TypeExpr::Primitive(PrimitiveKind::Boolean),
            OAType::Integer(integer) => match &integer.format {
                VariantOrUnknownOrEmpty::Item(IntegerFormat::Int64) => {
                    TypeExpr::Primitive(PrimitiveKind::Integer64)
                }
                _ => TypeExpr::Primitive(PrimitiveKind::Integer32),
            },
            OAType::Number(number) => match &number.format {
                VariantOrUnknownOrEmpty::Item(NumberFormat::Double) => {
                    TypeExpr::Primitive(PrimitiveKind::NumberDouble)
                }
                _ => TypeExpr::Primitive(PrimitiveKind::NumberFloat),
            },
            OAType::String(string) => TypeExpr::Primitive(map_string_format(&string.format)),
            OAType::Array(array) => {
                let items = array
                    .items
                    .as_ref()
                    .map(|schema| self.lower_boxed_schema_ref(schema))
                    .unwrap_or_else(|| TypeExpr::Primitive(PrimitiveKind::String));
                TypeExpr::Array(Box::new(items))
            }
            OAType::Object(object) => TypeExpr::Map {
                value: Box::new(TypeExpr::Primitive(PrimitiveKind::String)),
                additional: self.additional_behavior(&object.additional_properties),
            },
        }
    }
}

fn reference_to_model(reference: &str) -> TypeExpr {
    reference
        .split('/')
        .next_back()
        .map(|name| TypeExpr::Reference(ModelId(name.to_string())))
        .unwrap_or_else(|| TypeExpr::Primitive(PrimitiveKind::String))
}

fn map_string_format(format: &VariantOrUnknownOrEmpty<StringFormat>) -> PrimitiveKind {
    match format {
        VariantOrUnknownOrEmpty::Item(StringFormat::Byte) => PrimitiveKind::StringByte,
        VariantOrUnknownOrEmpty::Item(StringFormat::Binary) => PrimitiveKind::StringBinary,
        VariantOrUnknownOrEmpty::Item(StringFormat::Date) => PrimitiveKind::StringDate,
        VariantOrUnknownOrEmpty::Item(StringFormat::DateTime) => PrimitiveKind::StringDateTime,
        VariantOrUnknownOrEmpty::Item(StringFormat::Password) => PrimitiveKind::StringPassword,
        _ => PrimitiveKind::String,
    }
}
