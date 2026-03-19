use std::{cell::RefCell, collections::BTreeMap};

use openapiv3::{Header, OpenAPI, Parameter, ReferenceOr, RequestBody, Response, Schema};

use crate::{
    ir::{ApiIr, Model, ModelId, Operation, SpecInfo},
    resolver::{OperationEntry, ResolveResult, ResolvedSpec},
};

mod models;
mod operations;
mod types;

pub trait Resolver {
    type OperationIter<'a>: Iterator<Item = OperationEntry<'a>> + 'a
    where
        Self: 'a;
    type SchemaIter<'a>: Iterator<Item = (&'a str, &'a Schema)> + 'a
    where
        Self: 'a;

    fn spec(&self) -> &OpenAPI;
    fn operations(&self) -> Self::OperationIter<'_>;
    fn schemas(&self) -> Self::SchemaIter<'_>;
    fn resolve_parameter<'a>(
        &'a self,
        parameter: &'a ReferenceOr<Parameter>,
    ) -> ResolveResult<'a, Parameter>;
    fn resolve_request_body<'a>(
        &'a self,
        body: &'a ReferenceOr<RequestBody>,
    ) -> ResolveResult<'a, RequestBody>;
    fn resolve_response<'a>(
        &'a self,
        response: &'a ReferenceOr<Response>,
    ) -> ResolveResult<'a, Response>;
    fn resolve_header<'a>(&'a self, header: &'a ReferenceOr<Header>) -> ResolveResult<'a, Header>;
    fn resolve_schema<'a>(&'a self, schema: &'a ReferenceOr<Schema>) -> ResolveResult<'a, Schema>;
}

#[derive(Debug, Clone)]
pub struct LoweredApi {
    pub info: SpecInfo,
    pub models: BTreeMap<ModelId, Model>,
    pub operations: Vec<Operation>,
    pub diagnostics: Vec<LoweringDiagnostic>,
}

impl LoweredApi {
    pub fn into_api_ir(self) -> ApiIr {
        ApiIr {
            info: self.info,
            models: self.models,
            operations: self.operations,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweringDiagnostic {
    pub kind: LoweringDiagnosticKind,
    pub target: LoweringDiagnosticTarget,
    pub reference: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoweringDiagnosticKind {
    MissingReference,
    UnsupportedReference,
    Cycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoweringDiagnosticTarget {
    Parameter,
    RequestBody,
    Response,
    Header,
    Schema,
}

pub struct TypeLowerer<'a, R> {
    resolver: &'a R,
    diagnostics: RefCell<Vec<LoweringDiagnostic>>,
}

impl<'a, R> TypeLowerer<'a, R> {
    pub fn new(resolver: &'a R) -> Self {
        Self {
            resolver,
            diagnostics: RefCell::new(Vec::new()),
        }
    }
}

impl<R: Resolver> TypeLowerer<'_, R> {
    pub fn lower_api(&self) -> LoweredApi {
        LoweredApi {
            info: self.build_spec_info(),
            models: self.build_models(),
            operations: self.build_operations(),
            diagnostics: self.diagnostics.replace(Vec::new()),
        }
    }

    fn build_spec_info(&self) -> SpecInfo {
        let spec = self.resolver.spec();
        SpecInfo {
            title: spec.info.title.clone(),
            version: spec.info.version.clone(),
            description: spec.info.description.clone(),
        }
    }

    pub(crate) fn resolve_or_diagnostic<'a, T>(
        &'a self,
        target: LoweringDiagnosticTarget,
        result: ResolveResult<'a, T>,
    ) -> Option<&'a T> {
        match result {
            ResolveResult::Resolved(value) => Some(value),
            ResolveResult::MissingReference { reference } => {
                self.diagnostics.borrow_mut().push(LoweringDiagnostic {
                    kind: LoweringDiagnosticKind::MissingReference,
                    target,
                    reference: reference.to_string(),
                });
                None
            }
            ResolveResult::UnsupportedReference { reference } => {
                self.diagnostics.borrow_mut().push(LoweringDiagnostic {
                    kind: LoweringDiagnosticKind::UnsupportedReference,
                    target,
                    reference: reference.to_string(),
                });
                None
            }
            ResolveResult::Cycle { reference } => {
                self.diagnostics.borrow_mut().push(LoweringDiagnostic {
                    kind: LoweringDiagnosticKind::Cycle,
                    target,
                    reference: reference.to_string(),
                });
                None
            }
        }
    }
}

pub fn build_lowered_api<R: Resolver>(resolver: &R) -> LoweredApi {
    TypeLowerer::new(resolver).lower_api()
}

pub fn build_api_ir<R: Resolver>(resolver: &R) -> ApiIr {
    build_lowered_api(resolver).into_api_ir()
}

impl<'spec> Resolver for ResolvedSpec<'spec> {
    type OperationIter<'a>
        = Box<dyn Iterator<Item = OperationEntry<'a>> + 'a>
    where
        Self: 'a;
    type SchemaIter<'a>
        = Box<dyn Iterator<Item = (&'a str, &'a Schema)> + 'a>
    where
        Self: 'a;

    fn spec(&self) -> &OpenAPI {
        ResolvedSpec::spec(self)
    }

    fn operations(&self) -> Self::OperationIter<'_> {
        Box::new(ResolvedSpec::operations(self))
    }

    fn schemas(&self) -> Self::SchemaIter<'_> {
        Box::new(ResolvedSpec::schemas(self))
    }

    fn resolve_parameter<'a>(
        &'a self,
        parameter: &'a ReferenceOr<Parameter>,
    ) -> ResolveResult<'a, Parameter> {
        ResolvedSpec::resolve_parameter_result(self, parameter)
    }

    fn resolve_request_body<'a>(
        &'a self,
        body: &'a ReferenceOr<RequestBody>,
    ) -> ResolveResult<'a, RequestBody> {
        ResolvedSpec::resolve_request_body_result(self, body)
    }

    fn resolve_response<'a>(
        &'a self,
        response: &'a ReferenceOr<Response>,
    ) -> ResolveResult<'a, Response> {
        ResolvedSpec::resolve_response_result(self, response)
    }

    fn resolve_header<'a>(&'a self, header: &'a ReferenceOr<Header>) -> ResolveResult<'a, Header> {
        ResolvedSpec::resolve_header_result(self, header)
    }

    fn resolve_schema<'a>(&'a self, schema: &'a ReferenceOr<Schema>) -> ResolveResult<'a, Schema> {
        ResolvedSpec::resolve_schema_result(self, schema)
    }
}
