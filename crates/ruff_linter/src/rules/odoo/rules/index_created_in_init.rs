use ruff_macros::{ViolationMetadata, derive_message_formats};
use ruff_python_ast::helpers::any_over_body;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_python_semantic::SemanticModel;
use ruff_text_size::Ranged;

use crate::Violation;
use crate::checkers::ast::Checker;
use crate::codes::Category;
use crate::rules::odoo::helpers::{dotted_name, is_odoo_model_class};
use crate::rules::odoo::settings::OdooVersion;

/// ## What it does
/// Checks for an index that an Odoo model creates by hand in `init` (or `_auto_init`), with
/// `tools.create_index(...)` or with `cr.execute("CREATE [UNIQUE] INDEX ...")`, when the
/// index could be declared on the model with `models.Index` or `models.UniqueIndex`.
///
/// ## Why is this bad?
/// Odoo 19.0 declares the indexes of a model as class attributes, next to its fields and its
/// `models.Constraint`s. The ORM collects them from every class that extends the model, so an
/// index declared in one module is inherited, overridden by name or neutralized by another
/// module, and it is created, rebuilt when its definition changes and dropped from the same
/// place as the rest of the schema. An index created in `init` is invisible to all of that:
/// it is re-checked on every update with its own `IF NOT EXISTS`, an extending module cannot
/// change it without copying the call, and the code has to guard by hand against the table
/// not existing.
///
/// Only the statements that are plain enough to be moved are reported: an index whose name,
/// table, expressions, method and `WHERE` clause are all literals, written directly in the
/// body of `init`. These are left alone because the attribute cannot replace them:
///
/// - calls built from variables, f-strings, loops or conditions (one index per field,
///   an index created only when `pg_trgm` is installed);
/// - indexes on a table that is not the model's own, and models without a table of their own
///   (`_auto = False`, `models.AbstractModel`, views and materialized views);
/// - `CREATE INDEX CONCURRENTLY`, which cannot run inside the transaction that updates the
///   schema, and any statement that does more than create one index;
/// - indexes created outside a model, such as in a `post_init_hook`.
///
/// ## Example
/// ```python
/// from odoo import models, tools
///
///
/// class ResPartner(models.Model):
///     _inherit = "res.partner"
///
///     def init(self):
///         super().init()
///         tools.create_index(
///             self.env.cr, "res_partner_ref_idx", self._table, ["ref"], where="ref IS NOT NULL"
///         )
/// ```
///
/// Use instead:
/// ```python
/// from odoo import models
///
///
/// class ResPartner(models.Model):
///     _inherit = "res.partner"
///
///     _ref_idx = models.Index("(ref) WHERE ref IS NOT NULL")
/// ```
///
/// ## Options
/// - `lint.odoo.odoo-version`
///
/// ## Notes
/// The rule only runs when [`odoo-version`](../settings.md#lint_odoo_odoo-version) is set to
/// 19.0 or later: `models.Index` does not exist in Odoo 18.0 and earlier, and suggesting it
/// there would break the model at import.
///
/// There is no automatic fix. The database name of an index declared with an attribute is
/// always `{table}_{attribute name without its leading underscore}`, so an index created in
/// `init` under any other name would be created a second time under the new one while the old
/// one stays in the database until it is dropped by a migration script, and the rewrite also
/// has to move the statement out of the method into the class body.
#[derive(ViolationMetadata)]
#[violation_metadata(preview_since = "0.17.0.37", category = Category::Restriction)]
pub(crate) struct IndexCreatedInInit {
    unique: bool,
    method: String,
}

impl Violation for IndexCreatedInInit {
    #[derive_message_formats]
    fn message(&self) -> String {
        let IndexCreatedInInit { unique, method } = self;
        let declaration = if *unique {
            "`models.UniqueIndex`"
        } else {
            "`models.Index`"
        };
        format!(
            "Index created in `{method}`, declare it with {declaration} on the model since Odoo 19.0"
        )
    }
}

/// The Odoo version that introduced `models.Index` and `models.UniqueIndex`.
const SINCE: OdooVersion = OdooVersion::new(19, 0);

/// ODW9507
pub(crate) fn index_created_in_init(checker: &Checker, class_def: &ast::StmtClassDef) {
    // `models.Index` does not exist up to 18.0, and without a configured version there is no
    // telling which Odoo the code runs on, so the advice would be wrong for part of the users.
    let Some(version) = checker.settings().odoo.odoo_version else {
        return;
    };
    if version < SINCE {
        return;
    }
    if !is_odoo_model_class(checker.semantic(), class_def) {
        return;
    }
    // No table of its own: a view, a materialized view or a mixin, whose indexes are not the
    // ones a declaration on this class would create.
    if has_no_table(checker.semantic(), class_def) {
        return;
    }
    let table = model_table(class_def);

    for stmt in &class_def.body {
        let Stmt::FunctionDef(function_def) = stmt else {
            continue;
        };
        let method = function_def.name.as_str();
        if !matches!(method, "init" | "_auto_init") {
            continue;
        }
        // A method that builds a view builds its indexes on the view.
        if any_over_body(&function_def.body, mentions_view) {
            continue;
        }
        for stmt in &function_def.body {
            let Stmt::Expr(ast::StmtExpr { value, .. }) = stmt else {
                continue;
            };
            let Expr::Call(call) = value.as_ref() else {
                continue;
            };
            if let Some(unique) = index_call(checker.semantic(), call, table.as_deref()) {
                checker.report_diagnostic(
                    IndexCreatedInInit {
                        unique,
                        method: method.to_string(),
                    },
                    call.range(),
                );
            }
        }
    }
}

/// Returns `true` when the class does not map to a table of its own.
fn has_no_table(semantic: &SemanticModel, class_def: &ast::StmtClassDef) -> bool {
    let is_abstract = class_def.arguments.as_deref().is_some_and(|arguments| {
        arguments.args.iter().any(|base| {
            matches!(
                semantic
                    .resolve_qualified_name(base)
                    .as_ref()
                    .map(ruff_python_ast::name::QualifiedName::segments),
                Some(["odoo", "models", "AbstractModel"])
            )
        })
    });
    is_abstract
        || class_def.body.iter().any(|stmt| {
            let Stmt::Assign(assign) = stmt else {
                return false;
            };
            assign
                .targets
                .iter()
                .any(|target| matches!(target, Expr::Name(name) if name.id == "_auto"))
                && matches!(
                    assign.value.as_ref(),
                    Expr::BooleanLiteral(ast::ExprBooleanLiteral { value: false, .. })
                )
        })
}

/// The table the model maps to, when the class says it: `_table`, or `_name` (or the single
/// model `_inherit` extends) with its dots turned into underscores. Several `_inherit`s do not
/// say it.
fn model_table(class_def: &ast::StmtClassDef) -> Option<String> {
    let value_of = |attribute: &str| {
        class_def.body.iter().find_map(|stmt| {
            let Stmt::Assign(assign) = stmt else {
                return None;
            };
            if !assign
                .targets
                .iter()
                .any(|target| matches!(target, Expr::Name(name) if name.id == attribute))
            {
                return None;
            }
            match assign.value.as_ref() {
                Expr::StringLiteral(literal) => Some(literal.value.to_str().to_string()),
                _ => None,
            }
        })
    };
    value_of("_table").or_else(|| {
        value_of("_name")
            .or_else(|| value_of("_inherit"))
            .map(|name| name.replace('.', "_"))
    })
}

/// Returns `true` for a string literal that creates or drops a view.
fn mentions_view(expr: &Expr) -> bool {
    let Expr::StringLiteral(literal) = expr else {
        return false;
    };
    literal.value.to_str().to_ascii_uppercase().contains("VIEW")
}

/// When `call` creates one index that a declaration could replace, returns whether it is
/// unique.
fn index_call(semantic: &SemanticModel, call: &ast::ExprCall, table: Option<&str>) -> Option<bool> {
    if let Some(name) = semantic
        .resolve_qualified_name(&call.func)
        .as_ref()
        .map(ruff_python_ast::name::QualifiedName::segments)
    {
        return match name {
            ["odoo", "tools", "create_index"] | ["odoo", "tools", "sql", "create_index"] => {
                create_index_call(call, table, false)
            }
            ["odoo", "tools", "create_unique_index"]
            | ["odoo", "tools", "sql", "create_unique_index"] => {
                create_index_call(call, table, true)
            }
            _ => None,
        };
    }
    execute_call(call, table)
}

/// The argument of `call` at `position` or passed as `name`.
fn argument<'a>(call: &'a ast::ExprCall, position: usize, name: &str) -> Option<&'a Expr> {
    call.arguments.find_argument_value(name, position)
}

fn string_of(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::StringLiteral(literal) => Some(literal.value.to_str()),
        _ => None,
    }
}

/// `tools.create_index(cr, "name", self._table, ["col"], ...)` where every part is a literal.
fn create_index_call(call: &ast::ExprCall, table: Option<&str>, unique: bool) -> Option<bool> {
    // `create_index(cr, indexname, tablename, expressions, method='btree', where='', *,
    // comment=None, unique=False)`; the deprecated `create_unique_index` takes the first four.
    string_of(argument(call, 1, "indexname")?)?;
    match argument(call, 2, "tablename")? {
        Expr::StringLiteral(literal) => {
            if table.is_some_and(|table| table != literal.value.to_str()) {
                return None;
            }
        }
        table_expr if dotted_name(table_expr).as_deref() == Some("self._table") => {}
        _ => return None,
    }
    let (Expr::List(ast::ExprList { elts, .. }) | Expr::Tuple(ast::ExprTuple { elts, .. })) =
        argument(call, 3, "expressions")?
    else {
        return None;
    };
    if elts.is_empty() || !elts.iter().all(|elt| string_of(elt).is_some()) {
        return None;
    }
    if let Some(method) = argument(call, 4, "method") {
        string_of(method)?;
    }
    if let Some(r#where) = argument(call, 5, "where") {
        string_of(r#where)?;
    }
    // A comment would be replaced by the definition that `models.Index` stores as comment.
    if call.arguments.find_keyword("comment").is_some() {
        return None;
    }
    let mut unique = unique;
    if let Some(keyword) = call.arguments.find_keyword("unique") {
        match &keyword.value {
            Expr::BooleanLiteral(ast::ExprBooleanLiteral { value, .. }) => unique |= *value,
            _ => return None,
        }
    }
    Some(unique)
}

/// `cr.execute("CREATE [UNIQUE] INDEX [IF NOT EXISTS] name ON table ...")`, one statement.
fn execute_call(call: &ast::ExprCall, table: Option<&str>) -> Option<bool> {
    let Expr::Attribute(ast::ExprAttribute { value, attr, .. }) = call.func.as_ref() else {
        return None;
    };
    if attr.as_str() != "execute"
        || !matches!(
            dotted_name(value).as_deref(),
            Some("cr" | "self.env.cr" | "self._cr" | "self.cr" | "self.env.cr._obj")
        )
    {
        return None;
    }
    // Only the statement itself: parameters would make the text depend on values.
    let [sql] = call.arguments.args.as_ref() else {
        return None;
    };
    if !call.arguments.keywords.is_empty() {
        return None;
    }
    let sql = string_of(sql)?;
    let sql = sql.trim().trim_end_matches(';').trim_end();
    if sql.contains(';') || sql.contains('%') {
        return None;
    }
    let mut words = sql.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("CREATE") {
        return None;
    }
    let mut word = words.next()?;
    let unique = word.eq_ignore_ascii_case("UNIQUE");
    if unique {
        word = words.next()?;
    }
    if !word.eq_ignore_ascii_case("INDEX") {
        return None;
    }
    // `CONCURRENTLY` cannot run inside the transaction that updates the schema.
    let mut word = words.next()?;
    if word.eq_ignore_ascii_case("IF") {
        if !(words.next()?.eq_ignore_ascii_case("NOT")
            && words.next()?.eq_ignore_ascii_case("EXISTS"))
        {
            return None;
        }
        word = words.next()?;
    }
    // The index name, then `ON <table>`.
    if word.eq_ignore_ascii_case("ON") || word.eq_ignore_ascii_case("CONCURRENTLY") {
        return None;
    }
    if !words.next()?.eq_ignore_ascii_case("ON") {
        return None;
    }
    let on_table = words.next()?;
    // A table that is not the model's own, or one the class does not say, cannot be matched
    // to the model: the declaration builds `{table}_{name}` on the model's table.
    let on_table = on_table
        .split('(')
        .next()
        .unwrap_or(on_table)
        .trim_matches('"');
    let table = table?;
    (on_table == table).then_some(unique)
}
