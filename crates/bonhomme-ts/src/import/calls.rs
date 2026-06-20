use crate::scanner::stable_import_uuid;
use bonhomme_core::Operation;
use oxc_ast::ast::{
    Argument, Expression, ForStatementInit, Function, Statement, VariableDeclaration,
};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum CallTarget {
    Free(String),
    This(String),
}

pub(super) type CallsBySymbol = BTreeMap<Uuid, Vec<CallTarget>>;

struct ImportedSymbol {
    id: Uuid,
    parent_id: Option<Uuid>,
    calls: Vec<CallTarget>,
}

#[derive(Default)]
pub(super) struct ImportIndexes {
    symbols: Vec<ImportedSymbol>,
    name_index: BTreeMap<String, Vec<Uuid>>,
    sibling_index: BTreeMap<(Uuid, String), Uuid>,
}

impl ImportIndexes {
    pub(super) fn index_created_symbol(
        &mut self,
        operation: &Operation,
        calls_by_symbol: &CallsBySymbol,
    ) {
        let Operation::CreateSymbol {
            symbol_id,
            parent_id,
            name,
            ..
        } = operation
        else {
            return;
        };
        self.symbols.push(ImportedSymbol {
            id: *symbol_id,
            parent_id: *parent_id,
            calls: calls_by_symbol.get(symbol_id).cloned().unwrap_or_default(),
        });
        self.name_index
            .entry(name.clone())
            .or_default()
            .push(*symbol_id);
        if let Some(parent_id) = parent_id {
            self.sibling_index
                .entry((*parent_id, name.clone()))
                .or_insert(*symbol_id);
        }
    }
}

pub(super) fn import_references(indexes: &ImportIndexes) -> Vec<Operation> {
    let mut seen = BTreeSet::new();
    let mut operations = Vec::new();

    for symbol in &indexes.symbols {
        for call in &symbol.calls {
            let target = match call {
                CallTarget::This(name) => symbol.parent_id.and_then(|parent_id| {
                    indexes
                        .sibling_index
                        .get(&(parent_id, name.clone()))
                        .copied()
                }),
                CallTarget::Free(name) => indexes
                    .name_index
                    .get(name)
                    .and_then(|ids| (ids.len() == 1).then_some(ids[0])),
            };

            let Some(target_id) = target else {
                continue;
            };
            if target_id == symbol.id || !seen.insert((symbol.id, target_id, "calls".to_string())) {
                continue;
            }
            operations.push(Operation::CreateReference {
                reference_id: stable_import_uuid(&format!(
                    "reference:{}:{}:calls",
                    symbol.id, target_id
                )),
                from_symbol_id: symbol.id,
                to_symbol_id: target_id,
                kind: "calls".to_string(),
            });
        }
    }

    operations
}

pub(super) fn collect_function_calls(function: &Function<'_>) -> Vec<CallTarget> {
    let mut calls = Vec::new();
    if let Some(body) = &function.body {
        for statement in &body.statements {
            collect_statement_calls(statement, &mut calls);
        }
    }
    calls.sort();
    calls.dedup();
    calls
}

fn collect_statement_calls(statement: &Statement<'_>, calls: &mut Vec<CallTarget>) {
    match statement {
        Statement::BlockStatement(block) => {
            for statement in &block.body {
                collect_statement_calls(statement, calls);
            }
        }
        Statement::ExpressionStatement(statement) => {
            collect_expression_calls(&statement.expression, calls)
        }
        Statement::ReturnStatement(statement) => {
            if let Some(argument) = &statement.argument {
                collect_expression_calls(argument, calls);
            }
        }
        Statement::IfStatement(statement) => {
            collect_expression_calls(&statement.test, calls);
            collect_statement_calls(&statement.consequent, calls);
            if let Some(alternate) = &statement.alternate {
                collect_statement_calls(alternate, calls);
            }
        }
        Statement::WhileStatement(statement) => {
            collect_expression_calls(&statement.test, calls);
            collect_statement_calls(&statement.body, calls);
        }
        Statement::DoWhileStatement(statement) => {
            collect_statement_calls(&statement.body, calls);
            collect_expression_calls(&statement.test, calls);
        }
        Statement::ForStatement(statement) => {
            if let Some(init) = &statement.init {
                collect_for_init_calls(init, calls);
            }
            if let Some(test) = &statement.test {
                collect_expression_calls(test, calls);
            }
            if let Some(update) = &statement.update {
                collect_expression_calls(update, calls);
            }
            collect_statement_calls(&statement.body, calls);
        }
        Statement::ForInStatement(statement) => {
            collect_expression_calls(&statement.right, calls);
            collect_statement_calls(&statement.body, calls);
        }
        Statement::ForOfStatement(statement) => {
            collect_expression_calls(&statement.right, calls);
            collect_statement_calls(&statement.body, calls);
        }
        Statement::VariableDeclaration(declaration) => collect_variable_calls(declaration, calls),
        _ => {}
    }
}

fn collect_for_init_calls(init: &ForStatementInit<'_>, calls: &mut Vec<CallTarget>) {
    match init {
        ForStatementInit::VariableDeclaration(declaration) => {
            collect_variable_calls(declaration, calls)
        }
        ForStatementInit::CallExpression(expression) => {
            collect_call_expression_calls(expression, calls)
        }
        _ => {}
    }
}

fn collect_variable_calls(declaration: &VariableDeclaration<'_>, calls: &mut Vec<CallTarget>) {
    for declarator in &declaration.declarations {
        if let Some(init) = &declarator.init {
            collect_expression_calls(init, calls);
        }
    }
}

fn collect_expression_calls(expression: &Expression<'_>, calls: &mut Vec<CallTarget>) {
    match expression {
        Expression::CallExpression(call) => collect_call_expression_calls(call, calls),
        Expression::StaticMemberExpression(member) => {
            collect_expression_calls(&member.object, calls)
        }
        Expression::ComputedMemberExpression(member) => {
            collect_expression_calls(&member.object, calls);
            collect_expression_calls(&member.expression, calls);
        }
        Expression::ParenthesizedExpression(expression) => {
            collect_expression_calls(&expression.expression, calls);
        }
        Expression::BinaryExpression(expression) => {
            collect_expression_calls(&expression.left, calls);
            collect_expression_calls(&expression.right, calls);
        }
        Expression::LogicalExpression(expression) => {
            collect_expression_calls(&expression.left, calls);
            collect_expression_calls(&expression.right, calls);
        }
        Expression::ConditionalExpression(expression) => {
            collect_expression_calls(&expression.test, calls);
            collect_expression_calls(&expression.consequent, calls);
            collect_expression_calls(&expression.alternate, calls);
        }
        Expression::AssignmentExpression(expression) => {
            collect_expression_calls(&expression.right, calls)
        }
        Expression::TemplateLiteral(template) => {
            for expression in &template.expressions {
                collect_expression_calls(expression, calls);
            }
        }
        Expression::TaggedTemplateExpression(template) => {
            collect_expression_calls(&template.tag, calls);
            for expression in &template.quasi.expressions {
                collect_expression_calls(expression, calls);
            }
        }
        _ => {}
    }
}

fn collect_call_expression_calls(
    call: &oxc_ast::ast::CallExpression<'_>,
    calls: &mut Vec<CallTarget>,
) {
    if let Some(target) = call_target(&call.callee) {
        calls.push(target);
    }
    collect_expression_calls(&call.callee, calls);
    for argument in &call.arguments {
        collect_argument_calls(argument, calls);
    }
}

fn collect_argument_calls(argument: &Argument<'_>, calls: &mut Vec<CallTarget>) {
    match argument {
        Argument::CallExpression(call) => collect_call_expression_calls(call, calls),
        Argument::Identifier(_) => {}
        Argument::StaticMemberExpression(member) => collect_expression_calls(&member.object, calls),
        Argument::SpreadElement(spread) => collect_expression_calls(&spread.argument, calls),
        _ => {}
    }
}

fn call_target(callee: &Expression<'_>) -> Option<CallTarget> {
    match callee {
        Expression::Identifier(identifier) => Some(CallTarget::Free(identifier.name.to_string())),
        Expression::StaticMemberExpression(member)
            if matches!(member.object, Expression::ThisExpression(_)) =>
        {
            Some(CallTarget::This(member.property.name.to_string()))
        }
        Expression::ParenthesizedExpression(expression) => call_target(&expression.expression),
        _ => None,
    }
}
