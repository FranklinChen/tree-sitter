//! The rule trees the grammar contract publishes, and the paths each one
//! flattens to, for the proof the contract writer runs against the final
//! productions.
//!
//! A typed traversal presents a node's children in the grammar's structure (a
//! sequence with optional members, not every combination of them), so the
//! contract publishes each variable's rule tree as it stands just before
//! flattening: tokens extracted, repeats expanded into auxiliary symbols,
//! symbols interned.
//!
//! Three transformations are applied, each to state what the parse tables use
//! rather than what the grammar text says:
//!
//! - **Inlining.** Tree-sitter inlines only flattened productions; here every
//!   reference to an `inline` variable is replaced by that variable's tree.
//!   `process_inlines` gives the referencing step's alias and field precedence
//!   over the inlined steps' own, while flattening gives an inner alias or
//!   field precedence over an outer one, so the splice strips aliases (fields)
//!   inside an inlined body whenever an alias (field) is active at the
//!   reference.
//! - **Dead paths.** A path through an empty `choice()` matches nothing, and
//!   flattening drops it. The splice prunes such alternatives (and sequences
//!   containing one) before following any inline reference in them, so an
//!   inline cycle on a dead path is never entered.
//! - **Lexical context.** `reserved` contexts and precedence-only metadata
//!   change how the parser lexes and chooses, not the shape of the tree, and
//!   a reserved context does not carry across an inline boundary, so neither
//!   is published.
//!
//! Each tree's paths are collected with flattening's own enumeration
//! (`flatten_variable`), without flattening's validity checks (those already
//! ran on the real grammar). One rule differs from flattening's, because it is
//! inlining's: an inlined production that requires the end of input is kept
//! only where nothing follows it, and its requirement merges with the
//! reference's, so a path is live when every `eof()` step is in its trailing
//! run, and those steps become one end-of-input requirement.
//!
//! That rule admits every path the real passes keep, and one kind they drop:
//! flattening also requires an `eof()` to be its own rule's last step, which
//! the spliced tree cannot see when an inline reference that may match
//! nothing follows it. Such a path fails the proof, so the contract is
//! refused, never written with a production the parser does not have.

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    extract_tokens::ExtractedGrammarMeta,
    flatten_grammar::{FlattenGrammarError, FlattenState, flatten_variable},
};
use crate::{
    grammars::InputGrammar,
    rules::{Alias, MetadataParams, Rule, RuleId, RulePool, Symbol, SymbolType},
    strpool::StrId,
};

/// The rule trees the contract publishes, with what each flattens to.
#[derive(Debug, Clone)]
pub struct ContractStructure {
    /// Each syntax variable's tree and paths, by variable index.
    pub variables: Vec<VariableStructure>,
    /// Each lexical variable's source tree, in extraction order.
    pub terminals: Vec<TerminalSource>,
}

/// A syntax variable's tree, with the paths it flattens to.
#[derive(Debug, Clone)]
pub struct VariableStructure {
    /// The tree, inline references spliced, dead paths pruned. A tree that
    /// matches nothing is an empty choice.
    pub rule: StructuredRule,
    /// The paths the tree flattens to, by inlining's end-of-input rule, with
    /// each step's alias as the tree states it.
    pub paths: FxHashSet<ProductionKey>,
}

/// A lexical variable's name and source tree (a string, a pattern, or a
/// token's body).
#[derive(Debug, Clone)]
pub struct TerminalSource {
    pub name: StrId,
    pub rule: StructuredRule,
}

/// A variable's rule tree as the contract publishes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuredRule {
    Blank,
    Eof,
    Symbol(Symbol),
    String(StrId),
    Pattern(StrId, StrId),
    NamedSymbol(StrId),
    Seq(Vec<Self>),
    Choice(Vec<Self>),
    Repeat(Box<Self>),
    Metadata {
        alias: Option<Alias>,
        field: Option<StrId>,
        is_token: bool,
        is_main_token: bool,
        rule: Box<Self>,
    },
}

/// Why the structured rules could not be built.
#[derive(Debug, Error, Serialize, Deserialize, PartialEq, Eq)]
pub enum StructureError {
    #[error("Inline rule `{0}` reaches itself through inline references")]
    InlineCycle(String),
    #[error(transparent)]
    Flatten(#[from] FlattenGrammarError),
}

/// One step as the contract states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StepKey {
    pub symbol: Symbol,
    pub alias: Option<Alias>,
    pub field: Option<StrId>,
}

/// A production as the proof compares it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProductionKey {
    pub steps: Vec<StepKey>,
    /// The production completes only at the end of input.
    pub requires_eof: bool,
}

/// The structured rule of every variable, with inline references spliced and
/// dead paths pruned, and the paths each flattens to.
pub fn structured_rules(
    g: &InputGrammar,
    meta: &ExtractedGrammarMeta,
) -> Result<ContractStructure, StructureError> {
    let mut pool = g.pool.clone();
    let mut splicer = Splicer {
        source: g,
        inline: &meta.inline,
        active: Vec::new(),
    };
    // The spliced trees hold no reserved contexts, so flattening never looks
    // a reserved-word set up.
    let no_reserved_sets = FxHashMap::default();
    let mut state = FlattenState::default();
    let mut variables = Vec::with_capacity(g.variables.len());
    for variable in &g.variables {
        // A tree that matches nothing becomes an empty choice, which
        // flattening reads as having no paths.
        let root = match splicer.splice(&mut pool, variable.root, Overrides::default())? {
            Some(root) => root,
            None => pool.choice(&[]),
        };
        let mut paths = FxHashSet::default();
        flatten_variable(&pool, &no_reserved_sets, root, &mut state, |st| {
            if let Some(key) = inlined_path_key(st) {
                paths.insert(key);
            }
        })?;
        variables.push(VariableStructure {
            rule: structured(&pool, root),
            paths,
        });
    }
    Ok(ContractStructure {
        variables,
        terminals: meta
            .lexical_variables
            .iter()
            .map(|t| TerminalSource {
                name: t.name,
                rule: structured(&g.pool, t.root),
            })
            .collect(),
    })
}

/// The key of the path just enumerated, by inlining's rule for `eof()`: live
/// when every `eof()` step is in the trailing run, those steps becoming one
/// end-of-input requirement.
fn inlined_path_key(st: &FlattenState) -> Option<ProductionKey> {
    let steps = st.path_steps();
    let end = Symbol::end();
    let content = steps
        .iter()
        .rposition(|step| step.symbol() != end)
        .map_or(0, |last| last + 1);
    if steps[..content].iter().any(|step| step.symbol() == end) {
        return None;
    }
    Some(ProductionKey {
        steps: steps[..content]
            .iter()
            .map(|s| StepKey {
                symbol: s.symbol(),
                alias: s.alias(),
                field: s.field(),
            })
            .collect(),
        requires_eof: content < steps.len(),
    })
}

/// Where an alias (or a field) stands at a position in a tree being spliced.
#[derive(Clone, Copy, Default)]
enum Override {
    /// None is set above this position.
    #[default]
    Unset,
    /// One is set above, and an inner one overrides it (flattening's rule).
    Set,
    /// One is set at an enclosing inline reference, and wins over every one
    /// in the inlined body (inlining's rule): strip them.
    Overridden,
}

impl Override {
    /// Entering an inline variable's body from a reference here.
    const fn enter_inline(self) -> Self {
        match self {
            Self::Unset => Self::Unset,
            Self::Set | Self::Overridden => Self::Overridden,
        }
    }

    /// Passing a metadata node that does (`present`) or does not set one.
    const fn enter_metadata(self, present: bool) -> Self {
        match (self, present) {
            (Self::Unset, true) => Self::Set,
            (state, _) => state,
        }
    }
}

/// Where the alias and the field stand at a position.
#[derive(Clone, Copy, Default)]
struct Overrides {
    alias: Override,
    field: Override,
}

/// Whether metadata shapes the tree. Precedence alone does not: it chooses
/// between trees, so it is not published.
const fn shapes_tree(params: &MetadataParams) -> bool {
    params.alias.is_some() || params.field.is_some() || params.is_token || params.is_main_token
}

/// Splices inline references into a copy of the grammar's rule pool.
struct Splicer<'g> {
    /// The grammar as it stands just after flattening. String ids are
    /// resolved through its pool; the splice never interns a string, so the
    /// copy's ids are the same.
    source: &'g InputGrammar,
    inline: &'g [Symbol],
    /// The inline variables being spliced on the current path.
    active: Vec<u32>,
}

impl Splicer<'_> {
    /// `id` with inline references spliced and dead alternatives pruned, or
    /// `None` when it matches nothing.
    fn splice(
        &mut self,
        pool: &mut RulePool,
        id: RuleId,
        overrides: Overrides,
    ) -> Result<Option<RuleId>, StructureError> {
        match pool.node(id) {
            Rule::Sym {
                kind: SymbolType::NonTerminal,
                index,
            } if self.inline.contains(&Symbol::non_terminal(index as usize)) => {
                if self.active.contains(&index) {
                    let name = self.source.variables[index as usize].name;
                    return Err(StructureError::InlineCycle(
                        self.source.pool.resolve(name).to_string(),
                    ));
                }
                self.active.push(index);
                let body = Overrides {
                    alias: overrides.alias.enter_inline(),
                    field: overrides.field.enter_inline(),
                };
                let root = self.source.variables[index as usize].root;
                let spliced = self.splice(pool, root, body);
                self.active.pop();
                spliced
            }
            Rule::Seq(range) => {
                // A sequence with a member that matches nothing matches
                // nothing; decide that before following any inline reference
                // in it.
                if pool
                    .child_slice(range)
                    .iter()
                    .any(|&child| matches_nothing(pool, child))
                {
                    return Ok(None);
                }
                let mut rebuilt = Vec::with_capacity(range.len as usize);
                for i in 0..range.len as usize {
                    let child = pool.child_slice(range)[i];
                    match self.splice(pool, child, overrides)? {
                        Some(child) => rebuilt.push(child),
                        None => return Ok(None),
                    }
                }
                Ok(Some(pool.seq(&rebuilt)))
            }
            Rule::Choice(range) => {
                let mut rebuilt = Vec::with_capacity(range.len as usize);
                for i in 0..range.len as usize {
                    let child = pool.child_slice(range)[i];
                    if let Some(child) = self.splice(pool, child, overrides)? {
                        rebuilt.push(child);
                    }
                }
                Ok((!rebuilt.is_empty()).then(|| pool.choice(&rebuilt)))
            }
            Rule::Metadata { params, rule } => {
                let mut params = pool.params(params);
                if matches!(overrides.alias, Override::Overridden) {
                    params.alias = None;
                }
                if matches!(overrides.field, Override::Overridden) {
                    params.field = None;
                }
                let inner = Overrides {
                    alias: overrides.alias.enter_metadata(params.alias.is_some()),
                    field: overrides.field.enter_metadata(params.field.is_some()),
                };
                let Some(rule) = self.splice(pool, rule, inner)? else {
                    return Ok(None);
                };
                // Dropped here, not only when the tree is published, so
                // alternatives that differ only in precedence are one
                // alternative to `RulePool::choice`.
                if !shapes_tree(&params) {
                    return Ok(Some(rule));
                }
                let params = pool.push_params(params);
                Ok(Some(pool.push_node(Rule::Metadata { params, rule })))
            }
            // A reserved context changes how a token is lexed, not the tree,
            // and does not carry across an inline boundary.
            Rule::Reserved { rule, .. } => self.splice(pool, rule, overrides),
            Rule::Repeat(rule) => Ok(self
                .splice(pool, rule, overrides)?
                .map(|rule| pool.push_node(Rule::Repeat(rule)))),
            Rule::Blank
            | Rule::Eof
            | Rule::String(_)
            | Rule::Pattern(_, _)
            | Rule::NamedSymbol(_)
            | Rule::Sym { .. } => Ok(Some(id)),
        }
    }
}

/// Whether `id` matches nothing whatever its inline references hold: an empty
/// choice, or a construct every path through which meets one.
fn matches_nothing(pool: &RulePool, id: RuleId) -> bool {
    match pool.node(id) {
        Rule::Choice(range) => pool
            .child_slice(range)
            .iter()
            .all(|&child| matches_nothing(pool, child)),
        Rule::Seq(range) => pool
            .child_slice(range)
            .iter()
            .any(|&child| matches_nothing(pool, child)),
        Rule::Metadata { rule, .. } | Rule::Reserved { rule, .. } | Rule::Repeat(rule) => {
            matches_nothing(pool, rule)
        }
        Rule::Blank
        | Rule::Eof
        | Rule::String(_)
        | Rule::Pattern(_, _)
        | Rule::NamedSymbol(_)
        | Rule::Sym { .. } => false,
    }
}

/// The tree at `id`, without precedence or reserved contexts (a terminal's
/// source tree is not spliced, so they are dropped here too).
fn structured(pool: &RulePool, id: RuleId) -> StructuredRule {
    let all = |range| {
        pool.child_slice(range)
            .iter()
            .map(|&c| structured(pool, c))
            .collect()
    };
    match pool.node(id) {
        Rule::Blank => StructuredRule::Blank,
        Rule::Eof => StructuredRule::Eof,
        Rule::Sym { kind, index } => StructuredRule::Symbol(Symbol { kind, index }),
        Rule::String(value) => StructuredRule::String(value),
        Rule::Pattern(value, flags) => StructuredRule::Pattern(value, flags),
        Rule::NamedSymbol(name) => StructuredRule::NamedSymbol(name),
        Rule::Seq(range) => StructuredRule::Seq(all(range)),
        Rule::Choice(range) => StructuredRule::Choice(all(range)),
        Rule::Repeat(rule) => StructuredRule::Repeat(Box::new(structured(pool, rule))),
        Rule::Metadata { params, rule } => {
            let params = pool.params(params);
            let inner = structured(pool, rule);
            if !shapes_tree(&params) {
                return inner;
            }
            StructuredRule::Metadata {
                alias: params.alias,
                field: params.field,
                is_token: params.is_token,
                is_main_token: params.is_main_token,
                rule: Box::new(inner),
            }
        }
        Rule::Reserved { rule, .. } => structured(pool, rule),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ContractStructure, StructuredRule};
    use crate::{parse_grammar::parse_grammar, prepare_grammar::prepare_grammar_with_structure};

    fn structure(grammar: &serde_json::Value) -> ContractStructure {
        let mut diagnostics = Vec::new();
        let input = parse_grammar(&grammar.to_string(), &mut diagnostics).unwrap();
        let (_, structure) = prepare_grammar_with_structure(input, &mut diagnostics).unwrap();
        structure.unwrap()
    }

    fn count_aliases(rule: &StructuredRule) -> usize {
        match rule {
            StructuredRule::Metadata { alias, rule, .. } => {
                usize::from(alias.is_some()) + count_aliases(rule)
            }
            StructuredRule::Seq(members) | StructuredRule::Choice(members) => {
                members.iter().map(count_aliases).sum()
            }
            StructuredRule::Repeat(inner) => count_aliases(inner),
            StructuredRule::Blank
            | StructuredRule::Eof
            | StructuredRule::Symbol(_)
            | StructuredRule::String(_)
            | StructuredRule::Pattern(_, _)
            | StructuredRule::NamedSymbol(_) => 0,
        }
    }

    #[test]
    fn test_an_aliased_inline_reference_overrides_the_inlined_aliases() {
        // `source` aliases its reference to the inline rule `pair`, whose own
        // body aliases `a`; `process_inlines` lets the reference win.
        let grammar = json!({
            "name": "inline_alias",
            "rules": {
                "source": { "type": "ALIAS", "value": "outer", "named": true,
                            "content": { "type": "SYMBOL", "name": "pair" } },
                "pair": { "type": "SEQ", "members": [
                    { "type": "ALIAS", "value": "inner", "named": true,
                      "content": { "type": "SYMBOL", "name": "a" } },
                    { "type": "SYMBOL", "name": "b" }
                ]},
                "a": { "type": "STRING", "value": "a" },
                "b": { "type": "STRING", "value": "b" }
            },
            "inline": ["pair"]
        });
        let structure = structure(&grammar);
        assert_eq!(
            count_aliases(&structure.variables[0].rule),
            1,
            "{:?}",
            structure.variables[0].rule
        );
    }

    #[test]
    fn test_an_inline_cycle_on_a_dead_path_is_never_entered() {
        // `_e`'s recursive alternative passes through an empty choice, so it
        // matches nothing and flattening never sees the self-reference.
        let grammar = json!({
            "name": "dead_cycle",
            "rules": {
                "source": { "type": "SYMBOL", "name": "_e" },
                "_e": { "type": "CHOICE", "members": [
                    { "type": "STRING", "value": "a" },
                    { "type": "SEQ", "members": [
                        { "type": "CHOICE", "members": [] },
                        { "type": "SYMBOL", "name": "_e" }
                    ]}
                ]}
            },
            "inline": ["_e"]
        });
        let structure = structure(&grammar);
        assert_eq!(structure.variables[0].paths.len(), 1);
    }

    #[test]
    fn test_inlined_end_of_input_merges_with_the_reference() {
        // `_eol` may be `eof()`; inlined at the end of a production that ends
        // in `eof()` too, the two requirements merge (`process_inlines`), so
        // both alternatives are live.
        let grammar = json!({
            "name": "eof_merge",
            "rules": {
                "source": { "type": "SEQ", "members": [
                    { "type": "SYMBOL", "name": "stmt" },
                    { "type": "SYMBOL", "name": "_eol" },
                    { "type": "EOF" }
                ]},
                "stmt": { "type": "STRING", "value": "s" },
                "_eol": { "type": "CHOICE", "members": [
                    { "type": "STRING", "value": "\n" }, { "type": "EOF" }
                ]}
            },
            "inline": ["_eol"]
        });
        let structure = structure(&grammar);
        assert_eq!(
            structure.variables[0].paths.len(),
            2,
            "{:?}",
            structure.variables[0].paths
        );
        assert!(
            structure.variables[0]
                .paths
                .iter()
                .all(|key| key.requires_eof)
        );
    }
}
