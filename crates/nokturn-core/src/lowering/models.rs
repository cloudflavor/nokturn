use std::collections::BTreeMap;

use openapiv3::{Schema as OASchema, SchemaKind as OASchemaKind, Type as OAType};

use crate::ir::{Field, FieldMetadata, Model, ModelId, ModelKind};

use super::{Resolver, TypeLowerer};

impl<R: Resolver> TypeLowerer<'_, R> {
    pub(super) fn build_models(&self) -> BTreeMap<ModelId, Model> {
        let mut models = BTreeMap::new();

        for (name, schema) in self.resolver.schemas() {
            let id = ModelId(name.to_string());
            models.insert(id.clone(), self.lower_model(name, schema));
        }

        models
    }

    pub fn lower_model(&self, name: &str, schema: &OASchema) -> Model {
        let docs = schema.schema_data.description.clone();
        let deprecated = schema.schema_data.deprecated;
        let kind = match &schema.schema_kind {
            OASchemaKind::Type(OAType::Object(object)) => ModelKind::Struct {
                fields: object
                    .properties
                    .iter()
                    .map(|(prop_name, prop_schema)| Field {
                        name: prop_name.clone(),
                        json_name: prop_name.clone(),
                        ty: self.lower_boxed_schema_ref(prop_schema),
                        required: object.required.contains(prop_name),
                        metadata: FieldMetadata::default(),
                    })
                    .collect(),
                additional: self.additional_behavior(&object.additional_properties),
            },
            _ => ModelKind::Alias(self.lower_schema_type(schema)),
        };

        Model {
            name: name.to_string(),
            docs,
            deprecated,
            kind,
        }
    }
}
