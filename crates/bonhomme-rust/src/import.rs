use crate::{
    ids::{
        field_id, file_id, function_id, impl_id, method_id, reference_id, trait_method_id, type_id,
        value_id, variant_id,
    },
    model::{CallTarget, Declaration, Member, ParsedCrate, ParsedFile, RustMethod},
};
use anyhow::{Context, Result};
use bonhomme_core::{Operation, RenderedFile};
use quote::ToTokens;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use syn::{
    Expr, ExprCall, ExprMethodCall, Fields, ImplItem, Item, ItemConst, ItemEnum, ItemFn, ItemImpl,
    ItemStatic, ItemStruct, ItemTrait, ItemType, TraitItem, Type, Visibility, visit::Visit,
};
use uuid::Uuid;

const CALLS_KIND: &str = "calls";

#[derive(Default)]
pub(crate) struct ImportIndexes {
    pub(crate) types_by_name: BTreeMap<String, Vec<Uuid>>,
    pub(crate) functions_by_name: BTreeMap<String, Vec<Uuid>>,
    pub(crate) methods_by_key: BTreeMap<(Uuid, String), Uuid>,
    pub(crate) methods_by_name: BTreeMap<String, Vec<Uuid>>,
    pub(crate) calls: BTreeMap<Uuid, Vec<CallTarget>>,
}

pub fn import_rust_files(files: &[RenderedFile]) -> Result<Vec<Operation>> {
    let parsed = parse_rust_files(files)?;
    operations_from_parsed_crate(&parsed)
}

pub(crate) fn parse_rust_files(files: &[RenderedFile]) -> Result<ParsedCrate> {
    let mut parsed = ParsedCrate::default();
    for file in files {
        let syntax = syn::parse_file(&file.content)
            .with_context(|| format!("{} is not valid Rust", file.path))?;
        parsed.files.push(parse_file(&file.path, syntax));
    }
    parsed
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(parsed)
}

pub(crate) fn operations_from_parsed_crate(parsed: &ParsedCrate) -> Result<Vec<Operation>> {
    let mut indexes = ImportIndexes::default();
    index_crate(parsed, &mut indexes);

    let mut operations = Vec::new();
    for file in &parsed.files {
        operations.push(file_operation(file));
    }
    for file in &parsed.files {
        operations.extend(non_method_operations(file, &mut indexes)?);
    }
    for file in &parsed.files {
        operations.extend(method_operations(file, &mut indexes)?);
    }
    operations.extend(reference_operations(&indexes));
    Ok(operations)
}

fn parse_file(path: &str, syntax: syn::File) -> ParsedFile {
    let mut file = ParsedFile {
        path: path.to_string(),
        ..ParsedFile::default()
    };

    for (index, item) in syntax.items.into_iter().enumerate() {
        match item {
            Item::Use(item) => file.uses.push(tokens(&item)),
            Item::Struct(item) => file.declarations.push(parse_struct(item)),
            Item::Enum(item) => file.declarations.push(parse_enum(item)),
            Item::Trait(item) => file.declarations.push(parse_trait(item)),
            Item::Fn(item) => file.declarations.push(parse_function(item)),
            Item::Const(item) => file.declarations.push(parse_const(item)),
            Item::Static(item) => file.declarations.push(parse_static(item)),
            Item::Type(item) => file.declarations.push(parse_type_alias(item)),
            Item::Impl(item) => file.declarations.push(parse_impl(item)),
            other => file.declarations.push(parse_raw_item(index, other)),
        }
    }

    file
}

fn parse_struct(item: ItemStruct) -> Declaration {
    Declaration {
        kind: "struct".to_string(),
        name: item.ident.to_string(),
        declaration: struct_declaration(&item),
        fields: fields(&item.ident.to_string(), &item.fields),
        ..Declaration::default()
    }
}

fn parse_enum(item: ItemEnum) -> Declaration {
    Declaration {
        kind: "enum".to_string(),
        name: item.ident.to_string(),
        declaration: enum_declaration(&item),
        variants: item
            .variants
            .iter()
            .map(|variant| Member {
                name: variant.ident.to_string(),
                declaration: tokens(variant),
            })
            .collect(),
        ..Declaration::default()
    }
}

fn parse_trait(item: ItemTrait) -> Declaration {
    Declaration {
        kind: "trait".to_string(),
        name: item.ident.to_string(),
        declaration: trait_declaration(&item),
        trait_methods: item
            .items
            .iter()
            .filter_map(|trait_item| match trait_item {
                TraitItem::Fn(method) => Some(RustMethod {
                    name: method.sig.ident.to_string(),
                    signature: tokens(&method.sig),
                    body: method.default.as_ref().map(block_body),
                    impl_type: None,
                    impl_header: None,
                    calls: method
                        .default
                        .as_ref()
                        .map(calls_in_block)
                        .unwrap_or_default(),
                }),
                _ => None,
            })
            .collect(),
        ..Declaration::default()
    }
}

fn parse_function(item: ItemFn) -> Declaration {
    Declaration {
        kind: "function".to_string(),
        name: item.sig.ident.to_string(),
        signature: Some(signature(&item.vis, &item.sig)),
        body: Some(block_body(&item.block)),
        calls: calls_in_block(&item.block),
        ..Declaration::default()
    }
}

fn parse_const(item: ItemConst) -> Declaration {
    Declaration {
        kind: "const".to_string(),
        name: item.ident.to_string(),
        declaration: tokens(&item),
        ..Declaration::default()
    }
}

fn parse_static(item: ItemStatic) -> Declaration {
    Declaration {
        kind: "static".to_string(),
        name: item.ident.to_string(),
        declaration: tokens(&item),
        ..Declaration::default()
    }
}

fn parse_type_alias(item: ItemType) -> Declaration {
    Declaration {
        kind: "type".to_string(),
        name: item.ident.to_string(),
        declaration: tokens(&item),
        ..Declaration::default()
    }
}

fn parse_impl(item: ItemImpl) -> Declaration {
    let impl_type = type_name(&item.self_ty);
    let impl_header = impl_header(&item);
    let methods = item
        .items
        .iter()
        .filter_map(|impl_item| match impl_item {
            ImplItem::Fn(method) => Some(RustMethod {
                name: method.sig.ident.to_string(),
                signature: signature(&method.vis, &method.sig),
                body: Some(block_body(&method.block)),
                impl_type: impl_type.clone(),
                impl_header: Some(impl_header.clone()),
                calls: calls_in_block(&method.block),
            }),
            _ => None,
        })
        .collect();

    Declaration {
        kind: "impl".to_string(),
        name: impl_header.clone(),
        declaration: impl_header.clone(),
        impl_type,
        impl_header: Some(impl_header),
        methods,
        ..Declaration::default()
    }
}

fn parse_raw_item(index: usize, item: Item) -> Declaration {
    Declaration {
        kind: "raw".to_string(),
        name: format!("item-{index:04}"),
        declaration: tokens(&item),
        ..Declaration::default()
    }
}

fn index_crate(parsed: &ParsedCrate, indexes: &mut ImportIndexes) {
    for file in &parsed.files {
        for declaration in &file.declarations {
            match declaration.kind.as_str() {
                "struct" | "enum" | "trait" => {
                    indexes
                        .types_by_name
                        .entry(declaration.name.clone())
                        .or_default()
                        .push(type_id(&file.path, &declaration.name));
                }
                "function" => {
                    indexes
                        .functions_by_name
                        .entry(declaration.name.clone())
                        .or_default()
                        .push(function_id(&file.path, &declaration.name));
                }
                _ => {}
            }
        }
    }
}

fn file_operation(file: &ParsedFile) -> Operation {
    Operation::CreateSymbol {
        symbol_id: file_id(&file.path),
        parent_id: None,
        kind: "file".to_string(),
        name: file_name(&file.path),
        body: None,
        metadata: json!({
            "handler": "rust",
            "path": file.path,
            "uses": file.uses,
        }),
    }
}

fn non_method_operations(file: &ParsedFile, indexes: &mut ImportIndexes) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    let file_id = file_id(&file.path);
    for declaration in &file.declarations {
        match declaration.kind.as_str() {
            "struct" | "enum" | "trait" => {
                operations.extend(type_operations(file_id, file, declaration, indexes));
            }
            "function" => operations.push(function_operation(file_id, file, declaration, indexes)),
            "const" | "static" | "type" | "raw" => {
                operations.push(value_operation(file_id, file, declaration));
            }
            "impl" => {
                if unique_type_id(indexes, declaration.impl_type.as_deref()).is_none() {
                    operations.extend(impl_operations(file_id, file, declaration, indexes)?);
                }
            }
            _ => {}
        }
    }
    Ok(operations)
}

fn type_operations(
    file_id: Uuid,
    file: &ParsedFile,
    declaration: &Declaration,
    indexes: &mut ImportIndexes,
) -> Vec<Operation> {
    let symbol_id = type_id(&file.path, &declaration.name);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(file_id),
        kind: declaration.kind.clone(),
        name: declaration.name.clone(),
        body: None,
        metadata: json!({
            "declaration": declaration.declaration,
            "path": file.path,
        }),
    }];

    operations.extend(member_operations(
        symbol_id,
        "field",
        &declaration.fields,
        field_id,
    ));
    operations.extend(member_operations(
        symbol_id,
        "variant",
        &declaration.variants,
        variant_id,
    ));
    for method in &declaration.trait_methods {
        let method_id = trait_method_id(symbol_id, &method.name);
        indexes
            .methods_by_key
            .insert((symbol_id, method.name.clone()), method_id);
        indexes
            .methods_by_name
            .entry(method.name.clone())
            .or_default()
            .push(method_id);
        indexes.calls.insert(method_id, method.calls.clone());
        operations.push(Operation::CreateSymbol {
            symbol_id: method_id,
            parent_id: Some(symbol_id),
            kind: "method".to_string(),
            name: method.name.clone(),
            body: method.body.clone(),
            metadata: json!({
                "signature": method.signature,
                "path": file.path,
            }),
        });
    }
    operations
}

fn member_operations(
    parent_id: Uuid,
    kind: &str,
    members: &[Member],
    id_for: fn(Uuid, &str) -> Uuid,
) -> Vec<Operation> {
    members
        .iter()
        .map(|member| Operation::CreateSymbol {
            symbol_id: id_for(parent_id, &member.name),
            parent_id: Some(parent_id),
            kind: kind.to_string(),
            name: member.name.clone(),
            body: None,
            metadata: json!({"declaration": member.declaration}),
        })
        .collect()
}

fn function_operation(
    file_id: Uuid,
    file: &ParsedFile,
    declaration: &Declaration,
    indexes: &mut ImportIndexes,
) -> Operation {
    let symbol_id = function_id(&file.path, &declaration.name);
    indexes.calls.insert(symbol_id, declaration.calls.clone());
    Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(file_id),
        kind: "function".to_string(),
        name: declaration.name.clone(),
        body: declaration.body.clone(),
        metadata: json!({
            "signature": declaration.signature.as_deref().unwrap_or(""),
            "path": file.path,
        }),
    }
}

fn value_operation(file_id: Uuid, file: &ParsedFile, declaration: &Declaration) -> Operation {
    Operation::CreateSymbol {
        symbol_id: value_id(&file.path, &declaration.kind, &declaration.name),
        parent_id: Some(file_id),
        kind: declaration.kind.clone(),
        name: declaration.name.clone(),
        body: None,
        metadata: json!({
            "declaration": declaration.declaration,
            "path": file.path,
        }),
    }
}

fn impl_operations(
    file_id: Uuid,
    file: &ParsedFile,
    declaration: &Declaration,
    indexes: &mut ImportIndexes,
) -> Result<Vec<Operation>> {
    let header = declaration
        .impl_header
        .as_deref()
        .context("Rust impl declaration missing header")?;
    let impl_id = impl_id(&file.path, header);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id: impl_id,
        parent_id: Some(file_id),
        kind: "impl".to_string(),
        name: header.to_string(),
        body: None,
        metadata: json!({
            "declaration": header,
            "path": file.path,
        }),
    }];
    for method in &declaration.methods {
        operations.push(method_operation(impl_id, file, method, indexes));
    }
    Ok(operations)
}

fn method_operations(file: &ParsedFile, indexes: &mut ImportIndexes) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    for declaration in &file.declarations {
        if declaration.kind != "impl" {
            continue;
        }
        let Some(parent_id) = unique_type_id(indexes, declaration.impl_type.as_deref()) else {
            continue;
        };
        for method in &declaration.methods {
            operations.push(method_operation(parent_id, file, method, indexes));
        }
    }
    Ok(operations)
}

fn method_operation(
    parent_id: Uuid,
    file: &ParsedFile,
    method: &RustMethod,
    indexes: &mut ImportIndexes,
) -> Operation {
    let header = method.impl_header.as_deref().unwrap_or("");
    let symbol_id = method_id(parent_id, header, &method.name);
    indexes
        .methods_by_key
        .insert((parent_id, method.name.clone()), symbol_id);
    indexes
        .methods_by_name
        .entry(method.name.clone())
        .or_default()
        .push(symbol_id);
    indexes.calls.insert(symbol_id, method.calls.clone());
    Operation::CreateSymbol {
        symbol_id,
        parent_id: Some(parent_id),
        kind: "method".to_string(),
        name: method.name.clone(),
        body: method.body.clone(),
        metadata: json!({
            "signature": method.signature,
            "implHeader": header,
            "implType": method.impl_type.as_deref().unwrap_or(""),
            "path": file.path,
        }),
    }
}

pub(crate) fn reference_operations(indexes: &ImportIndexes) -> Vec<Operation> {
    let mut seen = BTreeSet::new();
    let mut operations = Vec::new();
    for (from_symbol_id, calls) in &indexes.calls {
        for call in calls {
            let Some(to_symbol_id) = resolve_call(indexes, call) else {
                continue;
            };
            if to_symbol_id == *from_symbol_id
                || !seen.insert((*from_symbol_id, to_symbol_id, CALLS_KIND))
            {
                continue;
            }
            operations.push(Operation::CreateReference {
                reference_id: reference_id(*from_symbol_id, to_symbol_id, CALLS_KIND),
                from_symbol_id: *from_symbol_id,
                to_symbol_id,
                kind: CALLS_KIND.to_string(),
            });
        }
    }
    operations
}

fn resolve_call(indexes: &ImportIndexes, call: &CallTarget) -> Option<Uuid> {
    match call.kind.as_str() {
        "function" => unique(indexes.functions_by_name.get(&call.name)?),
        "method" => resolve_method_call(indexes, call),
        _ => None,
    }
}

fn resolve_method_call(indexes: &ImportIndexes, call: &CallTarget) -> Option<Uuid> {
    if let Some(receiver) = &call.receiver
        && let Some(parent_id) = unique(indexes.types_by_name.get(receiver)?)
        && let Some(method_id) = indexes.methods_by_key.get(&(parent_id, call.name.clone()))
    {
        return Some(*method_id);
    }
    unique(indexes.methods_by_name.get(&call.name)?)
}

fn unique_type_id(indexes: &ImportIndexes, name: Option<&str>) -> Option<Uuid> {
    unique(indexes.types_by_name.get(name?)?)
}

fn unique(ids: &[Uuid]) -> Option<Uuid> {
    (ids.len() == 1).then_some(ids[0])
}

fn fields(type_name: &str, fields: &Fields) -> Vec<Member> {
    match fields {
        Fields::Named(fields) => fields
            .named
            .iter()
            .filter_map(|field| {
                let name = field.ident.as_ref()?.to_string();
                Some(Member {
                    name,
                    declaration: tokens(field),
                })
            })
            .collect(),
        Fields::Unnamed(fields) => fields
            .unnamed
            .iter()
            .enumerate()
            .map(|(index, field)| Member {
                name: format!("{type_name}.{index}"),
                declaration: tokens(field),
            })
            .collect(),
        Fields::Unit => Vec::new(),
    }
}

fn calls_in_block(block: &syn::Block) -> Vec<CallTarget> {
    let mut visitor = CallVisitor::default();
    visitor.visit_block(block);
    visitor.calls.sort();
    visitor.calls.dedup();
    visitor.calls
}

#[derive(Default)]
struct CallVisitor {
    calls: Vec<CallTarget>,
}

impl<'ast> Visit<'ast> for CallVisitor {
    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Expr::Path(path) = &*node.func
            && let Some(call) = call_from_path(path)
        {
            self.calls.push(call);
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        self.calls.push(CallTarget {
            kind: "method".to_string(),
            name: node.method.to_string(),
            receiver: None,
        });
        syn::visit::visit_expr_method_call(self, node);
    }
}

fn call_from_path(path: &syn::ExprPath) -> Option<CallTarget> {
    if path.qself.is_some() {
        return None;
    }
    let mut segments = path.path.segments.iter().collect::<Vec<_>>();
    let name = segments.pop()?.ident.to_string();
    let receiver = segments.last().map(|segment| segment.ident.to_string());
    Some(CallTarget {
        kind: if receiver.is_some() {
            "method".to_string()
        } else {
            "function".to_string()
        },
        name,
        receiver,
    })
}

fn signature(vis: &Visibility, sig: &syn::Signature) -> String {
    format!("{} {}", tokens(vis), tokens(sig))
        .trim()
        .to_string()
}

fn block_body(block: &syn::Block) -> String {
    block
        .stmts
        .iter()
        .map(tokens)
        .collect::<Vec<_>>()
        .join("\n")
}

fn struct_declaration(item: &ItemStruct) -> String {
    format!(
        "{} struct {} {}",
        tokens(&item.vis),
        item.ident,
        tokens(&item.generics)
    )
    .trim()
    .to_string()
}

fn enum_declaration(item: &ItemEnum) -> String {
    format!(
        "{} enum {} {}",
        tokens(&item.vis),
        item.ident,
        tokens(&item.generics)
    )
    .trim()
    .to_string()
}

fn trait_declaration(item: &ItemTrait) -> String {
    format!(
        "{} trait {} {}",
        tokens(&item.vis),
        item.ident,
        tokens(&item.generics)
    )
    .trim()
    .to_string()
}

fn impl_header(item: &ItemImpl) -> String {
    let generics = tokens(&item.generics);
    let self_ty = tokens(&item.self_ty);
    match &item.trait_ {
        Some((_, path, _)) => format!("impl {generics} {} for {self_ty}", tokens(path))
            .trim()
            .to_string(),
        None => format!("impl {generics} {self_ty}").trim().to_string(),
    }
}

fn type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        Type::Reference(reference) => type_name(&reference.elem),
        Type::Ptr(pointer) => type_name(&pointer.elem),
        Type::Paren(paren) => type_name(&paren.elem),
        Type::Group(group) => type_name(&group.elem),
        _ => None,
    }
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn tokens<T: ToTokens>(node: T) -> String {
    node.to_token_stream().to_string()
}
