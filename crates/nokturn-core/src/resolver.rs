// Copyright 2026 Cloudflavor GmbH

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::utils::{HttpMethod, decode_pointer_segment};
use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use indexmap::IndexMap;
use openapiv3::{
    AdditionalProperties, AnySchema, Callback, Example, Header, Link, LinkOperation, MediaType,
    OpenAPI, Operation as OAPIOperation, Parameter, ParameterSchemaOrContent, PathItem,
    ReferenceOr, RequestBody, Response, Responses, Schema, SchemaKind, SecurityScheme, Type,
};
use std::collections::{HashMap, HashSet};
use tracing::warn;

#[derive(Debug)]
pub(crate) struct ResolvedCache<'a> {
    paths: IndexMap<&'a str, &'a PathItem>,
    params: HashMap<&'a str, &'a Parameter>,
    schemas: HashMap<&'a str, &'a Schema>,
    responses: HashMap<&'a str, &'a Response>,
    headers: HashMap<&'a str, &'a Header>,
    callbacks: HashMap<&'a str, &'a Callback>,
    req_bodies: HashMap<RequestBodyKey<'a>, &'a RequestBody>,
    examples: HashMap<&'a str, &'a Example>,
    links: HashMap<&'a str, &'a Link>,
    security_schemes: HashMap<&'a str, &'a SecurityScheme>,
}

#[derive(Debug, Hash, Eq, PartialEq)]
enum RequestBodyKey<'a> {
    Component(&'a str),
    Operation { path: &'a str, method: HttpMethod },
}

#[derive(Clone, Copy)]
struct OperationLocator<'a> {
    path: &'a str,
    method: HttpMethod,
}

pub struct ResolvedSpec<'a> {
    spec: &'a OpenAPI,
    cache: ResolvedCache<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveResult<'a, T> {
    Resolved(&'a T),
    MissingReference { reference: &'a str },
    UnsupportedReference { reference: &'a str },
    Cycle { reference: &'a str },
}

impl<'a, T> ResolveResult<'a, T> {
    pub fn into_option(self) -> Option<&'a T> {
        match self {
            Self::Resolved(value) => Some(value),
            Self::MissingReference { .. }
            | Self::UnsupportedReference { .. }
            | Self::Cycle { .. } => None,
        }
    }
}

pub struct OperationEntry<'a> {
    pub path: &'a str,
    pub method: HttpMethod,
    pub operation: &'a OAPIOperation,
    pub path_item: &'a PathItem,
}

pub struct EagerResolver<'a> {
    spec: &'a OpenAPI,
    // resolved: Option<ResolvedSpec>,
    cache: ResolvedCache<'a>,
    visited_refs: HashSet<&'a str>,
    operation_ids: HashMap<&'a str, OperationLocator<'a>>,
}

impl<'a> EagerResolver<'a> {
    pub fn new(spec: &'a OpenAPI) -> Self {
        let paths_cap = spec.paths.paths.len();
        let (
            param_cap,
            schema_cap,
            response_cap,
            header_cap,
            callback_cap,
            rbody_cap,
            example_cap,
            link_cap,
            security_cap,
        ) = spec
            .components
            .as_ref()
            .map(|c| {
                (
                    c.parameters.len(),
                    c.schemas.len(),
                    c.responses.len(),
                    c.headers.len(),
                    c.callbacks.len(),
                    c.request_bodies.len(),
                    c.examples.len(),
                    c.links.len(),
                    c.security_schemes.len(),
                )
            })
            .unwrap_or_default();

        Self {
            spec,
            // resolved: None,
            cache: ResolvedCache {
                req_bodies: HashMap::with_capacity(rbody_cap),
                paths: IndexMap::with_capacity(paths_cap),
                params: HashMap::with_capacity(param_cap),
                schemas: HashMap::with_capacity(schema_cap),
                responses: HashMap::with_capacity(response_cap),
                headers: HashMap::with_capacity(header_cap),
                callbacks: HashMap::with_capacity(callback_cap),
                examples: HashMap::with_capacity(example_cap),
                links: HashMap::with_capacity(link_cap),
                security_schemes: HashMap::with_capacity(security_cap),
            },
            visited_refs: HashSet::new(),
            operation_ids: HashMap::new(),
        }
    }

    pub fn resolve(mut self) -> Result<ResolvedSpec<'a>> {
        self.resolve_component_schemas()
            .with_context(|| "failed to resolve component schemas")?;
        self.resolve_paths()
            .with_context(|| "failed to resolve paths")?;

        Ok(ResolvedSpec {
            spec: self.spec,
            cache: self.cache,
        })
    }

    fn resolve_component_schemas(&mut self) -> Result<()> {
        let Some(components) = self.spec.components.as_ref() else {
            return Ok(());
        };

        let schema_names: Vec<&'a str> = components.schemas.keys().map(String::as_str).collect();
        for name in schema_names {
            self.resolve_schema_component_ref(name)?;
        }

        Ok(())
    }

    fn resolve_paths(&mut self) -> Result<()> {
        for (name, path_item) in self.spec.paths.iter() {
            match path_item {
                ReferenceOr::Item(item) => self.cache_path_item(name.as_str(), item)?,
                ReferenceOr::Reference { reference } => {
                    self.resolve_path_alias(name.as_str(), reference)?;
                }
            }
        }
        Ok(())
    }

    fn cache_path_item(&mut self, path: &'a str, path_item: &'a PathItem) -> Result<()> {
        self.record_operation_ids(path, path_item);
        self.resolve_operation(path, path_item)?;
        self.cache.paths.insert(path, path_item);
        Ok(())
    }

    fn resolve_path_alias(&mut self, alias_path: &'a str, reference: &'a str) -> Result<()> {
        self.resolve_ref(reference)?;

        let target_path = match ComponentRef::parse(reference) {
            Some(ComponentRef::PathItem { path_segment }) => decode_pointer_segment(path_segment),
            _ => {
                return Err(anyhow!(
                    "path item reference did not resolve to a path item: {reference}"
                ));
            }
        };

        let target = self
            .cache
            .paths
            .get(target_path.as_str())
            .copied()
            .ok_or_else(|| anyhow!("resolved path cache missing target: {reference}"))?;

        self.record_operation_ids(alias_path, target);
        self.cache.paths.insert(alias_path, target);

        Ok(())
    }

    fn resolve_operation(&mut self, path: &'a str, path_item: &'a PathItem) -> Result<()> {
        for param in &path_item.parameters {
            self.resolve_parameter_or(param)?;
        }

        for (method, op) in iterate_operations(path_item) {
            self.resolve_operation_entry(path, method, op)?;
        }

        Ok(())
    }

    fn resolve_operation_entry(
        &mut self,
        path: &'a str,
        method: HttpMethod,
        operation: &'a OAPIOperation,
    ) -> Result<()> {
        for param in &operation.parameters {
            self.resolve_parameter_or(param)?;
        }
        self.resolve_responses(&operation.responses)?;
        self.resolve_req_body(path, method, operation.request_body.as_ref())?;
        for callback in operation.callbacks.values() {
            self.resolve_callback(callback)?;
        }

        Ok(())
    }

    fn record_operation_ids(&mut self, path: &'a str, path_item: &'a PathItem) {
        for (method, operation) in iterate_operations(path_item) {
            if let Some(operation_id) = operation.operation_id.as_deref() {
                self.operation_ids
                    .insert(operation_id, OperationLocator { path, method });
            }
        }
    }

    fn resolve_responses(&mut self, responses: &'a Responses) -> Result<()> {
        if let Some(default_resp) = responses.default.as_ref() {
            match default_resp {
                ReferenceOr::Item(item) => {
                    self.resolve_response(item)?;
                }
                ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref())?,
            }
        }

        for resp in responses.responses.values() {
            match resp {
                ReferenceOr::Item(item) => {
                    self.resolve_response(item)?;
                }
                ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref())?,
            }
        }

        Ok(())
    }

    fn resolve_response(&mut self, response: &'a Response) -> Result<()> {
        self.resolve_headers(response.headers.iter())?;
        for media_type in response.content.values() {
            self.resolve_media_type(media_type)?;
        }
        for link in response.links.values() {
            self.resolve_link_or(link)?;
        }
        Ok(())
    }

    fn resolve_headers<I>(&mut self, headers: I) -> Result<()>
    where
        I: Iterator<Item = (&'a String, &'a ReferenceOr<Header>)>,
    {
        for (_name, header) in headers {
            match header {
                ReferenceOr::Item(item) => {
                    self.resolve_header(item)?;
                }
                ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref())?,
            }
        }
        Ok(())
    }

    fn resolve_header(&mut self, header: &'a Header) -> Result<()> {
        match &header.format {
            ParameterSchemaOrContent::Schema(schema) => self.resolve_schema_ref(schema),
            ParameterSchemaOrContent::Content(content) => {
                for media in content.values() {
                    self.resolve_media_type(media)?;
                }
                Ok(())
            }
        }?;
        for example in header.examples.values() {
            self.resolve_example_or(example)?;
        }
        Ok(())
    }

    fn resolve_parameter_or(&mut self, parameter: &'a ReferenceOr<Parameter>) -> Result<()> {
        match parameter {
            ReferenceOr::Item(item) => self.resolve_parameter(item),
            ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref()),
        }
    }

    fn resolve_req_body(
        &mut self,
        path: &'a str,
        method: HttpMethod,
        req_body: Option<&'a ReferenceOr<RequestBody>>,
    ) -> Result<()> {
        let Some(req_body) = req_body else {
            return Ok(());
        };

        match req_body {
            ReferenceOr::Item(item) => {
                self.cache
                    .req_bodies
                    .insert(RequestBodyKey::Operation { path, method }, item);
                self.resolve_request_body(item)
            }
            ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref()),
        }
    }

    fn resolve_parameter(&mut self, parameter: &'a Parameter) -> Result<()> {
        let data = parameter.parameter_data_ref();
        match &data.format {
            ParameterSchemaOrContent::Schema(schema) => self.resolve_schema_ref(schema),
            ParameterSchemaOrContent::Content(content) => {
                for media in content.values() {
                    self.resolve_media_type(media)?;
                }
                Ok(())
            }
        }?;
        for example in data.examples.values() {
            self.resolve_example_or(example)?;
        }
        Ok(())
    }

    fn resolve_media_type(&mut self, media_type: &'a MediaType) -> Result<()> {
        if let Some(schema) = media_type.schema.as_ref() {
            self.resolve_schema_ref(schema)?;
        }
        for encoding in media_type.encoding.values() {
            self.resolve_headers(encoding.headers.iter())?;
        }
        for example in media_type.examples.values() {
            self.resolve_example_or(example)?;
        }
        Ok(())
    }

    fn resolve_request_body(&mut self, body: &'a RequestBody) -> Result<()> {
        for media_type in body.content.values() {
            self.resolve_media_type(media_type)?;
        }
        Ok(())
    }

    fn resolve_callback(&mut self, callback: &'a Callback) -> Result<()> {
        for (expression, path_item) in callback.iter() {
            self.resolve_operation(expression.as_str(), path_item)?;
        }
        Ok(())
    }

    fn resolve_example_or(&mut self, example: &'a ReferenceOr<Example>) -> Result<()> {
        match example {
            ReferenceOr::Item(_) => Ok(()),
            ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref()),
        }
    }

    fn resolve_link_or(&mut self, link: &'a ReferenceOr<Link>) -> Result<()> {
        match link {
            ReferenceOr::Item(item) => self.resolve_link(item),
            ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref()),
        }
    }

    fn resolve_link(&mut self, link: &'a Link) -> Result<()> {
        match &link.operation {
            LinkOperation::OperationRef(reference) => self.resolve_ref(reference.as_str()),
            LinkOperation::OperationId(operation_id) => {
                self.resolve_operation_id(operation_id.as_str())
            }
        }
    }

    fn resolve_schema_ref(&mut self, schema: &'a ReferenceOr<Schema>) -> Result<()> {
        match schema {
            ReferenceOr::Item(item) => self.resolve_schema(item),
            ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref()),
        }
    }

    fn resolve_boxed_schema_ref(&mut self, schema: &'a ReferenceOr<Box<Schema>>) -> Result<()> {
        match schema {
            ReferenceOr::Item(item) => self.resolve_schema(item.as_ref()),
            ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref()),
        }
    }

    fn resolve_schema(&mut self, schema: &'a Schema) -> Result<()> {
        match &schema.schema_kind {
            SchemaKind::Type(t) => self.resolve_type(t),
            SchemaKind::OneOf { one_of } => {
                for s in one_of {
                    self.resolve_schema_ref(s)?;
                }
                Ok(())
            }
            SchemaKind::AllOf { all_of } => {
                for s in all_of {
                    self.resolve_schema_ref(s)?;
                }
                Ok(())
            }
            SchemaKind::AnyOf { any_of } => {
                for s in any_of {
                    self.resolve_schema_ref(s)?;
                }
                Ok(())
            }
            SchemaKind::Not { not } => self.resolve_schema_ref(not.as_ref()),
            SchemaKind::Any(any) => self.resolve_any_schema(any),
        }
    }

    fn resolve_type(&mut self, schema_type: &'a Type) -> Result<()> {
        match schema_type {
            Type::Array(array) => {
                if let Some(items) = &array.items {
                    self.resolve_boxed_schema_ref(items)?;
                }
                Ok(())
            }
            Type::Object(object) => {
                for property in object.properties.values() {
                    self.resolve_boxed_schema_ref(property)?;
                }
                if let Some(additional) = &object.additional_properties
                    && let AdditionalProperties::Schema(schema) = additional
                {
                    self.resolve_schema_ref(schema.as_ref())?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn resolve_any_schema(&mut self, schema: &'a AnySchema) -> Result<()> {
        for property in schema.properties.values() {
            self.resolve_boxed_schema_ref(property)?;
        }
        if let Some(additional) = &schema.additional_properties
            && let AdditionalProperties::Schema(schema) = additional
        {
            self.resolve_schema_ref(schema.as_ref())?;
        }
        if let Some(items) = &schema.items {
            self.resolve_boxed_schema_ref(items)?;
        }
        for s in &schema.one_of {
            self.resolve_schema_ref(s)?;
        }
        for s in &schema.all_of {
            self.resolve_schema_ref(s)?;
        }
        for s in &schema.any_of {
            self.resolve_schema_ref(s)?;
        }
        if let Some(not) = &schema.not {
            self.resolve_schema_ref(not.as_ref())?;
        }
        Ok(())
    }

    fn resolve_parameter_ref(&mut self, name: &'a str) -> Result<()> {
        let comps = self
            .spec
            .components
            .as_ref()
            .ok_or_else(|| anyhow!("spec has no components"))?;

        match &comps.parameters[name] {
            ReferenceOr::Item(item) => {
                self.cache.params.insert(name, item);
                self.resolve_parameter(item)
            }
            ReferenceOr::Reference { reference } => {
                self.resolve_ref(reference.as_ref())?;
                let target_name = match ComponentRef::parse(reference.as_ref()) {
                    Some(ComponentRef::Parameter(target_name)) => target_name,
                    _ => {
                        return Err(anyhow!(
                            "parameter reference did not resolve to a parameter: {reference}"
                        ));
                    }
                };
                let target = self.cache.params.get(target_name).copied().ok_or_else(|| {
                    anyhow!("resolved parameter cache missing target: {reference}")
                })?;
                self.cache.params.insert(name, target);
                Ok(())
            }
        }
    }

    fn resolve_ref(&mut self, reference: &'a str) -> Result<()> {
        if !self.visited_refs.insert(reference) {
            return Ok(());
        }

        let result = match ComponentRef::parse(reference) {
            Some(ComponentRef::Parameter(name)) => self.resolve_parameter_ref(name),
            Some(ComponentRef::Response(name)) => self.resolve_response_ref(name),
            Some(ComponentRef::Header(name)) => self.resolve_header_ref(name),
            Some(ComponentRef::Schema(name)) => self.resolve_schema_component_ref(name),
            Some(ComponentRef::Callback(name)) => self.resolve_callback_ref(name),
            Some(ComponentRef::RequestBody(name)) => self.resolve_request_body_ref(name),
            Some(ComponentRef::Example(name)) => self.resolve_example_ref(name),
            Some(ComponentRef::Link(name)) => self.resolve_link_ref(name),
            Some(ComponentRef::SecurityScheme(name)) => self.resolve_security_scheme_ref(name),
            Some(ComponentRef::PathItem { path_segment }) => {
                self.resolve_path_item_ref(path_segment)
            }
            Some(ComponentRef::PathOperation {
                path_segment,
                method,
                has_extra,
            }) => self.resolve_path_operation_ref(path_segment, method, has_extra),
            None => {
                warn!("unresolved component reference: {reference}");
                Err(anyhow!("unresolved component reference: {reference} "))
            }
        };

        if result.is_err() {
            self.visited_refs.remove(reference);
        }

        result
    }

    fn resolve_response_ref(&mut self, name: &'a str) -> Result<()> {
        let comps = self
            .spec
            .components
            .as_ref()
            .ok_or_else(|| anyhow!("spec has no components"))?;

        match &comps.responses[name] {
            ReferenceOr::Item(item) => {
                self.cache.responses.insert(name, item);
                self.resolve_response(item)
            }
            ReferenceOr::Reference { reference } => {
                self.resolve_ref(reference.as_ref())?;
                let target_name = match ComponentRef::parse(reference.as_ref()) {
                    Some(ComponentRef::Response(target_name)) => target_name,
                    _ => {
                        return Err(anyhow!(
                            "response reference did not resolve to a response: {reference}"
                        ));
                    }
                };
                let target = self
                    .cache
                    .responses
                    .get(target_name)
                    .copied()
                    .ok_or_else(|| {
                        anyhow!("resolved response cache missing target: {reference}")
                    })?;
                self.cache.responses.insert(name, target);
                Ok(())
            }
        }
    }

    fn resolve_header_ref(&mut self, name: &'a str) -> Result<()> {
        let comps = self
            .spec
            .components
            .as_ref()
            .ok_or_else(|| anyhow!("spec has no components"))?;

        match &comps.headers[name] {
            ReferenceOr::Item(item) => {
                self.cache.headers.insert(name, item);
                self.resolve_header(item)
            }
            ReferenceOr::Reference { reference } => {
                self.resolve_ref(reference.as_ref())?;
                let target_name = match ComponentRef::parse(reference.as_ref()) {
                    Some(ComponentRef::Header(target_name)) => target_name,
                    _ => {
                        return Err(anyhow!(
                            "header reference did not resolve to a header: {reference}"
                        ));
                    }
                };
                let target = self
                    .cache
                    .headers
                    .get(target_name)
                    .copied()
                    .ok_or_else(|| anyhow!("resolved header cache missing target: {reference}"))?;
                self.cache.headers.insert(name, target);
                Ok(())
            }
        }
    }

    fn resolve_schema_component_ref(&mut self, name: &'a str) -> Result<()> {
        let comps = self
            .spec
            .components
            .as_ref()
            .ok_or_else(|| anyhow!("spec has no components"))?;

        match &comps.schemas[name] {
            ReferenceOr::Item(item) => {
                self.cache.schemas.insert(name, item);
                self.resolve_schema(item)
            }
            ReferenceOr::Reference { reference } => {
                self.resolve_ref(reference.as_ref())?;
                let target_name = match ComponentRef::parse(reference.as_ref()) {
                    Some(ComponentRef::Schema(target_name)) => target_name,
                    _ => {
                        return Err(anyhow!(
                            "schema reference did not resolve to a schema: {reference}"
                        ));
                    }
                };
                let target = self
                    .cache
                    .schemas
                    .get(target_name)
                    .copied()
                    .ok_or_else(|| anyhow!("resolved schema cache missing target: {reference}"))?;
                self.cache.schemas.insert(name, target);
                Ok(())
            }
        }
    }

    fn resolve_callback_ref(&mut self, name: &'a str) -> Result<()> {
        let comps = self
            .spec
            .components
            .as_ref()
            .ok_or_else(|| anyhow!("spec has no components"))?;

        match &comps.callbacks[name] {
            ReferenceOr::Item(item) => {
                self.cache.callbacks.insert(name, item);
                self.resolve_callback(item)
            }
            ReferenceOr::Reference { reference } => {
                self.resolve_ref(reference.as_ref())?;
                let target_name = match ComponentRef::parse(reference.as_ref()) {
                    Some(ComponentRef::Callback(target_name)) => target_name,
                    _ => {
                        return Err(anyhow!(
                            "callback reference did not resolve to a callback: {reference}"
                        ));
                    }
                };
                let target = self
                    .cache
                    .callbacks
                    .get(target_name)
                    .copied()
                    .ok_or_else(|| {
                        anyhow!("resolved callback cache missing target: {reference}")
                    })?;
                self.cache.callbacks.insert(name, target);
                Ok(())
            }
        }
    }

    fn resolve_request_body_ref(&mut self, name: &'a str) -> Result<()> {
        let comps = self
            .spec
            .components
            .as_ref()
            .ok_or_else(|| anyhow!("spec has no components"))?;

        match &comps.request_bodies[name] {
            ReferenceOr::Item(item) => {
                self.cache
                    .req_bodies
                    .insert(RequestBodyKey::Component(name), item);
                self.resolve_request_body(item)
            }
            ReferenceOr::Reference { reference } => {
                self.resolve_ref(reference.as_ref())?;
                let target_name = match ComponentRef::parse(reference.as_ref()) {
                    Some(ComponentRef::RequestBody(target_name)) => target_name,
                    _ => {
                        return Err(anyhow!(
                            "request body reference did not resolve to a request body: {reference}"
                        ));
                    }
                };
                let target = self
                    .cache
                    .req_bodies
                    .get(&RequestBodyKey::Component(target_name))
                    .copied()
                    .ok_or_else(|| {
                        anyhow!("resolved request body cache missing target: {reference}")
                    })?;
                self.cache
                    .req_bodies
                    .insert(RequestBodyKey::Component(name), target);
                Ok(())
            }
        }
    }

    fn resolve_example_ref(&mut self, name: &'a str) -> Result<()> {
        let comps = self
            .spec
            .components
            .as_ref()
            .ok_or_else(|| anyhow!("spec has no components"))?;

        match &comps.examples[name] {
            ReferenceOr::Item(item) => {
                self.cache.examples.insert(name, item);
                Ok(())
            }
            ReferenceOr::Reference { reference } => {
                self.resolve_ref(reference.as_ref())?;
                let target_name = match ComponentRef::parse(reference.as_ref()) {
                    Some(ComponentRef::Example(target_name)) => target_name,
                    _ => {
                        return Err(anyhow!(
                            "example reference did not resolve to an example: {reference}"
                        ));
                    }
                };
                let target = self
                    .cache
                    .examples
                    .get(target_name)
                    .copied()
                    .ok_or_else(|| anyhow!("resolved example cache missing target: {reference}"))?;
                self.cache.examples.insert(name, target);
                Ok(())
            }
        }
    }

    fn resolve_link_ref(&mut self, name: &'a str) -> Result<()> {
        let comps = self
            .spec
            .components
            .as_ref()
            .ok_or_else(|| anyhow!("spec has no components"))?;

        match &comps.links[name] {
            ReferenceOr::Item(item) => {
                self.cache.links.insert(name, item);
                self.resolve_link(item)
            }
            ReferenceOr::Reference { reference } => {
                self.resolve_ref(reference.as_ref())?;
                let target_name = match ComponentRef::parse(reference.as_ref()) {
                    Some(ComponentRef::Link(target_name)) => target_name,
                    _ => {
                        return Err(anyhow!(
                            "link reference did not resolve to a link: {reference}"
                        ));
                    }
                };
                let target =
                    self.cache.links.get(target_name).copied().ok_or_else(|| {
                        anyhow!("resolved link cache missing target: {reference}")
                    })?;
                self.cache.links.insert(name, target);
                Ok(())
            }
        }
    }

    fn resolve_security_scheme_ref(&mut self, name: &'a str) -> Result<()> {
        let comps = self
            .spec
            .components
            .as_ref()
            .ok_or_else(|| anyhow!("spec has no components"))?;

        match &comps.security_schemes[name] {
            ReferenceOr::Item(item) => {
                self.cache.security_schemes.insert(name, item);
                Ok(())
            }
            ReferenceOr::Reference { reference } => {
                self.resolve_ref(reference.as_ref())?;
                let target_name = match ComponentRef::parse(reference.as_ref()) {
                    Some(ComponentRef::SecurityScheme(target_name)) => target_name,
                    _ => {
                        return Err(anyhow!(
                            "security scheme reference did not resolve to a security scheme: {reference}"
                        ));
                    }
                };
                let target = self
                    .cache
                    .security_schemes
                    .get(target_name)
                    .copied()
                    .ok_or_else(|| {
                        anyhow!("resolved security scheme cache missing target: {reference}")
                    })?;
                self.cache.security_schemes.insert(name, target);
                Ok(())
            }
        }
    }

    fn resolve_operation_id(&mut self, operation_id: &'a str) -> Result<()> {
        let locator = self
            .operation_ids
            .get(operation_id)
            .ok_or_else(|| anyhow!("link references unknown operationId: {operation_id}"))?;

        let path_entry = self
            .spec
            .paths
            .paths
            .get(locator.path)
            .ok_or_else(|| anyhow!("operationId points to missing path: {}", locator.path))?;

        match path_entry {
            ReferenceOr::Item(item) => {
                self.resolve_single_operation(locator.method, locator.path, item)
            }
            ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref()),
        }
    }

    fn resolve_path_operation_ref(
        &mut self,
        path_segment: &'a str,
        method: HttpMethod,
        has_extra: bool,
    ) -> Result<()> {
        if has_extra {
            warn!("link operationRef has extra pointer segments");
        }

        let decoded_path = decode_pointer_segment(path_segment);
        let (path_name, path_item) = self
            .spec
            .paths
            .paths
            .get_key_value(decoded_path.as_str())
            .ok_or_else(|| anyhow!("link references unknown path: {decoded_path}"))?;

        match path_item {
            ReferenceOr::Item(item) => {
                self.resolve_single_operation(method, path_name.as_str(), item)
            }
            ReferenceOr::Reference { reference } => self.resolve_ref(reference.as_ref()),
        }
    }

    fn resolve_path_item_ref(&mut self, path_segment: &'a str) -> Result<()> {
        let decoded_path = decode_pointer_segment(path_segment);
        let (path_name, path_item) = self
            .spec
            .paths
            .paths
            .get_key_value(decoded_path.as_str())
            .ok_or_else(|| anyhow!("path item reference targets unknown path: {decoded_path}"))?;

        match path_item {
            ReferenceOr::Item(item) => self.cache_path_item(path_name.as_str(), item),
            ReferenceOr::Reference { reference } => {
                self.resolve_path_alias(path_name.as_str(), reference)
            }
        }
    }

    fn resolve_single_operation(
        &mut self,
        method: HttpMethod,
        path: &'a str,
        path_item: &'a PathItem,
    ) -> Result<()> {
        if let Some(operation) = find_operation_by_method(path_item, method) {
            for param in &path_item.parameters {
                self.resolve_parameter_or(param)?;
            }
            self.resolve_operation_entry(path, method, operation)?;
        }

        Ok(())
    }
}

fn iterate_operations(
    path_item: &PathItem,
) -> impl Iterator<Item = (HttpMethod, &openapiv3::Operation)> {
    [
        (HttpMethod::Get, path_item.get.as_ref()),
        (HttpMethod::Put, path_item.put.as_ref()),
        (HttpMethod::Post, path_item.post.as_ref()),
        (HttpMethod::Delete, path_item.delete.as_ref()),
        (HttpMethod::Patch, path_item.patch.as_ref()),
        (HttpMethod::Options, path_item.options.as_ref()),
        (HttpMethod::Head, path_item.head.as_ref()),
        (HttpMethod::Trace, path_item.trace.as_ref()),
    ]
    .into_iter()
    .filter_map(|(method, op)| op.map(|operation| (method, operation)))
}

fn find_operation_by_method(
    path_item: &PathItem,
    method: HttpMethod,
) -> Option<&openapiv3::Operation> {
    match method {
        HttpMethod::Get => path_item.get.as_ref(),
        HttpMethod::Put => path_item.put.as_ref(),
        HttpMethod::Post => path_item.post.as_ref(),
        HttpMethod::Delete => path_item.delete.as_ref(),
        HttpMethod::Patch => path_item.patch.as_ref(),
        HttpMethod::Options => path_item.options.as_ref(),
        HttpMethod::Head => path_item.head.as_ref(),
        HttpMethod::Trace => path_item.trace.as_ref(),
    }
}

impl<'a> ResolvedSpec<'a> {
    pub fn spec(&self) -> &'a OpenAPI {
        self.spec
    }

    pub fn operations(&self) -> impl Iterator<Item = OperationEntry<'a>> + '_ {
        self.cache.paths.iter().flat_map(|(path, path_item)| {
            let path = *path;
            let path_item = *path_item;

            iterate_operations(path_item).map(move |(method, operation)| OperationEntry {
                path,
                method,
                operation,
                path_item,
            })
        })
    }

    pub fn for_each_operation<F>(&self, mut f: F)
    where
        F: FnMut(OperationEntry<'a>),
    {
        for entry in self.operations() {
            f(entry);
        }
    }

    pub fn resolve_parameter(
        &self,
        parameter: &'a ReferenceOr<Parameter>,
    ) -> Option<&'a Parameter> {
        self.resolve_parameter_result(parameter).into_option()
    }

    pub fn resolve_parameter_result(
        &self,
        parameter: &'a ReferenceOr<Parameter>,
    ) -> ResolveResult<'a, Parameter> {
        match parameter {
            ReferenceOr::Item(item) => ResolveResult::Resolved(item),
            ReferenceOr::Reference { reference } => match ComponentRef::parse(reference) {
                Some(ComponentRef::Parameter(name)) => self
                    .cache
                    .params
                    .get(name)
                    .copied()
                    .map(ResolveResult::Resolved)
                    .unwrap_or(ResolveResult::MissingReference { reference }),
                _ => ResolveResult::UnsupportedReference { reference },
            },
        }
    }

    pub fn resolve_request_body(
        &self,
        body: &'a ReferenceOr<RequestBody>,
    ) -> Option<&'a RequestBody> {
        self.resolve_request_body_result(body).into_option()
    }

    pub fn resolve_request_body_result(
        &self,
        body: &'a ReferenceOr<RequestBody>,
    ) -> ResolveResult<'a, RequestBody> {
        match body {
            ReferenceOr::Item(item) => ResolveResult::Resolved(item),
            ReferenceOr::Reference { reference } => match ComponentRef::parse(reference) {
                Some(ComponentRef::RequestBody(name)) => self
                    .cache
                    .req_bodies
                    .get(&RequestBodyKey::Component(name))
                    .copied(),
                _ => None,
            }
            .map(ResolveResult::Resolved)
            .unwrap_or_else(|| match ComponentRef::parse(reference) {
                Some(ComponentRef::RequestBody(_)) => ResolveResult::MissingReference { reference },
                _ => ResolveResult::UnsupportedReference { reference },
            }),
        }
    }

    pub fn resolve_response(&self, response: &'a ReferenceOr<Response>) -> Option<&'a Response> {
        self.resolve_response_result(response).into_option()
    }

    pub fn resolve_response_result(
        &self,
        response: &'a ReferenceOr<Response>,
    ) -> ResolveResult<'a, Response> {
        match response {
            ReferenceOr::Item(item) => ResolveResult::Resolved(item),
            ReferenceOr::Reference { reference } => match ComponentRef::parse(reference) {
                Some(ComponentRef::Response(name)) => self
                    .cache
                    .responses
                    .get(name)
                    .copied()
                    .map(ResolveResult::Resolved)
                    .unwrap_or(ResolveResult::MissingReference { reference }),
                _ => ResolveResult::UnsupportedReference { reference },
            },
        }
    }

    pub fn resolve_header(&self, header: &'a ReferenceOr<Header>) -> Option<&'a Header> {
        self.resolve_header_result(header).into_option()
    }

    pub fn resolve_header_result(
        &self,
        header: &'a ReferenceOr<Header>,
    ) -> ResolveResult<'a, Header> {
        match header {
            ReferenceOr::Item(item) => ResolveResult::Resolved(item),
            ReferenceOr::Reference { reference } => match ComponentRef::parse(reference) {
                Some(ComponentRef::Header(name)) => self
                    .cache
                    .headers
                    .get(name)
                    .copied()
                    .map(ResolveResult::Resolved)
                    .unwrap_or(ResolveResult::MissingReference { reference }),
                _ => ResolveResult::UnsupportedReference { reference },
            },
        }
    }

    pub fn resolve_schema(&self, schema: &'a ReferenceOr<Schema>) -> Option<&'a Schema> {
        self.resolve_schema_result(schema).into_option()
    }

    pub fn resolve_schema_result(
        &self,
        schema: &'a ReferenceOr<Schema>,
    ) -> ResolveResult<'a, Schema> {
        match schema {
            ReferenceOr::Item(item) => ResolveResult::Resolved(item),
            ReferenceOr::Reference { reference } => match ComponentRef::parse(reference) {
                Some(ComponentRef::Schema(name)) => self
                    .cache
                    .schemas
                    .get(name)
                    .copied()
                    .map(ResolveResult::Resolved)
                    .unwrap_or(ResolveResult::MissingReference { reference }),
                _ => ResolveResult::UnsupportedReference { reference },
            },
        }
    }

    pub fn schemas(&self) -> impl Iterator<Item = (&'a str, &'a Schema)> + '_ {
        self.cache
            .schemas
            .iter()
            .map(|(name, schema)| (*name, *schema))
    }
}

enum ComponentRef<'a> {
    Parameter(&'a str),
    Response(&'a str),
    Header(&'a str),
    Schema(&'a str),
    Callback(&'a str),
    RequestBody(&'a str),
    Example(&'a str),
    Link(&'a str),
    SecurityScheme(&'a str),
    PathItem {
        path_segment: &'a str,
    },
    PathOperation {
        path_segment: &'a str,
        method: HttpMethod,
        has_extra: bool,
    },
}

impl<'a> ComponentRef<'a> {
    fn parse(reference: &'a str) -> Option<Self> {
        let pieces: Vec<_> = reference
            .trim_start_matches('#')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        match pieces.as_slice() {
            ["components", kind, name] => match *kind {
                "parameters" => Some(Self::Parameter(name)),
                "responses" => Some(Self::Response(name)),
                "schemas" => Some(Self::Schema(name)),
                "headers" => Some(Self::Header(name)),
                "callbacks" => Some(Self::Callback(name)),
                "requestBodies" => Some(Self::RequestBody(name)),
                "examples" => Some(Self::Example(name)),
                "links" => Some(Self::Link(name)),
                "securitySchemes" => Some(Self::SecurityScheme(name)),
                _ => None,
            },
            ["paths", path] => Some(Self::PathItem { path_segment: path }),
            ["paths", path, method, rest @ ..] => {
                method.parse().ok().map(|method| Self::PathOperation {
                    path_segment: path,
                    method,
                    has_extra: !rest.is_empty(),
                })
            }
            _ => None,
        }
    }
}
