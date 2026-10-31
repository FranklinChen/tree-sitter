//! The prepared grammar, published as a contract.
//!
//! `node-types.json` summarizes, for each node kind, which kinds may appear
//! beneath it. It does not say in what order, through which production, or
//! with which aliases and fields at each step, so a consumer that needs those
//! facts (a typed traversal generator, say) had to re-derive them from
//! `grammar.json`, re-implementing token extraction, repeat expansion,
//! inlining and alias application. This module writes the facts the generator
//! itself computed, from the same values `node-types.json` and the parse
//! tables are built from, so no consumer has to infer them.
//!
//! The contract is written only when asked for (`generate --grammar-contract`),
//! from a separate preparation of the grammar, so the parser and every other
//! output are produced exactly as without it.
//!
//! Each variable carries its final productions after inlining and its rule
//! tree (see `prepare_grammar::contract_structure`). The writer proves the
//! tree: the paths it flattens to must be exactly the variable's final
//! productions, step by step (symbol, alias, field) and in the end-of-input
//! requirement, and the contract is refused otherwise. Every step states the
//! alias it is shown under, so a default alias (which `extract_default_aliases`
//! removes from the steps it covers) is restored on each step it applies to.
//!
//! An auxiliary variable is always a repetition: `expand_repeats` builds every
//! one with `wrap_in_binary_tree`, either as a new `<rule>_repeatN` or by
//! converting a hidden rule whose body is a top-level repeat (which also leaves
//! `inline`). Its productions are one binary `[self, self]` production plus the
//! repeated elements. The contract states the elements only, meaning one or
//! more of them, joined by invisible binary subtrees, and checks the binary
//! production is there rather than assuming it.
//!
//! Every symbol is referenced as `{ "kind", "index" }` into the array its
//! kind names (`variables`, `terminals`, `external_tokens`), exactly as the
//! generator numbers them, so names never have to be matched.
//!
//! A variable's kind, inlining and supertype membership are three separate
//! facts in the generator, but only six combinations of them occur. The
//! contract states the combination as one `form`, decided by
//! [`variable_form`], so a consumer never meets a combination Tree-sitter does
//! not build.

use std::hash::BuildHasherDefault;

use indexmap::IndexSet;
use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    grammars::{InputGrammar, VariableType},
    node_types::{
        SuperTypeCycleError, VariableInfo, VariableInfoError, get_variable_info, node_kinds,
        supertype_subtypes,
    },
    prepare_grammar::{
        ContractStructure, MissingInlineExpansion, PrepareGrammarError, PreparedGrammar,
        ProductionKey, StepKey, StructureError, StructuredRule, VariableStructure,
        final_productions, prepare_grammar_with_structure,
    },
    rules::{Alias, Symbol, SymbolType},
    strpool::StrPool,
};

/// The contract's format: whose format it is, and its version. Changed on any
/// change a consumer must know.
const CONTRACT_FORMAT: &str = "tree-sitter-contract-fork/grammar-contract/4";

/// Why the contract could not be written.
#[derive(Debug, Error, Serialize, Deserialize)]
pub enum ContractError {
    #[error(transparent)]
    Prepare(#[from] PrepareGrammarError),
    #[error(transparent)]
    VariableInfo(#[from] VariableInfoError),
    #[error(transparent)]
    NodeTypes(#[from] SuperTypeCycleError),
    #[error(transparent)]
    Structure(#[from] StructureError),
    #[error(transparent)]
    Inlining(#[from] MissingInlineExpansion),
    #[error(
        "The rule tree of `{variable}` does not flatten to its productions: {tree_only} \
         path(s) only in the tree, {productions_only} production(s) only in the grammar"
    )]
    Unproven {
        variable: String,
        tree_only: usize,
        productions_only: usize,
    },
    #[error(
        "Internal error: the rule tree of auxiliary variable `{0}` is not a choice with one \
         plain `[self, self]` sequence; every repetition is"
    )]
    AuxiliaryStructure(String),
    #[error(
        "Internal error: variable `{variable}` is {form}, which Tree-sitter refuses before the \
         contract is written"
    )]
    VariableForm { variable: String, form: IllegalForm },
    #[error(
        "Internal error: external token `{0}` is auxiliary; Tree-sitter gives an external token \
         the visibility of its spelling"
    )]
    AuxiliaryExternal(String),
    #[error(
        "Internal error: the lexical grammar has {lexical} terminals but {sources} source \
         trees, or their names differ at index {index}"
    )]
    TerminalSources {
        lexical: usize,
        sources: usize,
        index: usize,
    },
    #[error("The grammar contract could not be serialized: {0}")]
    Serialize(String),
}

#[derive(Serialize)]
struct Contract<'a> {
    format: &'static str,
    /// The grammar's name, as `grammar.json` states it and the parser's
    /// `tree_sitter_<name>` function is named.
    name: &'a str,
    variables: Vec<VariableEntry<'a>>,
    terminals: Vec<TerminalEntry<'a>>,
    external_tokens: Vec<ExternalTokenEntry<'a>>,
    extras: Vec<SymbolRef>,
    word_token: Option<SymbolRef>,
    default_aliases: Vec<DefaultAlias<'a>>,
    /// Every node kind, named and anonymous, exactly as `node-types.json`
    /// lists them (same computation, same order).
    node_kinds: Vec<NodeTypeEntry<'a>>,
}

#[derive(Serialize)]
struct VariableEntry<'a> {
    name: &'a str,
    #[serde(flatten)]
    form: VariableForm<'a>,
    /// The rule tree just before flattening, inline references spliced; for
    /// a repetition, the repeated element alone.
    rule: RuleEntry<'a>,
    /// The final productions, each once; for a repetition, the elements'.
    productions: Vec<ProductionEntry<'a>>,
}

/// What a variable is: one variant for each combination of kind, inlining and
/// supertype membership that Tree-sitter builds, written as `"form"`.
#[derive(Serialize)]
#[serde(tag = "form", rename_all = "snake_case")]
enum VariableForm<'a> {
    /// A visible node named by the variable.
    Named,
    /// A visible anonymous node.
    Anonymous,
    /// No node; its children are placed in its parent.
    Hidden,
    /// Hidden, and listed in the grammar's `supertypes`.
    Supertype {
        /// Its subtypes as `node-types.json` lists them (same computation,
        /// same order).
        subtypes: Vec<NodeTypeEntry<'a>>,
    },
    /// Listed in the grammar's `inline`: replaced by its productions wherever
    /// it is referenced, so it never produces a node.
    Inline,
    /// An auxiliary variable, which is always a repetition: one or more of
    /// its element.
    Repetition,
}

/// A combination of a variable's kind, inlining and supertype membership that
/// names no form.
///
/// Tree-sitter rules out each before the contract is written:
/// a supertype entry for an inline rule is dropped and every supertype is made
/// hidden (`intern_symbols`), a hidden rule that becomes a repetition leaves
/// `inline` and no generated repetition is ever in it (`expand_repeats`), and a
/// supertype must have a single visible child, which a repetition never has
/// (`node_types`).
#[derive(Debug, Error, Serialize, Deserialize)]
pub enum IllegalForm {
    #[error("both inline and a supertype")]
    InlineSupertype,
    #[error("a visible supertype")]
    VisibleSupertype,
    #[error("an inline repetition")]
    InlineRepetition,
    #[error("a repetition and a supertype")]
    RepetitionSupertype,
}

/// The form of a variable of `kind`, inlined or not, with `subtypes` exactly
/// when it is a supertype.
fn variable_form(
    kind: VariableType,
    inline: bool,
    subtypes: Option<Vec<NodeTypeEntry<'_>>>,
) -> Result<VariableForm<'_>, IllegalForm> {
    match (kind, inline, subtypes) {
        (_, true, Some(_)) => Err(IllegalForm::InlineSupertype),
        (VariableType::Named | VariableType::Anonymous, false, Some(_)) => {
            Err(IllegalForm::VisibleSupertype)
        }
        (VariableType::Auxiliary, true, None) => Err(IllegalForm::InlineRepetition),
        (VariableType::Auxiliary, false, Some(_)) => Err(IllegalForm::RepetitionSupertype),
        (VariableType::Hidden, false, Some(subtypes)) => Ok(VariableForm::Supertype { subtypes }),
        (VariableType::Named | VariableType::Anonymous | VariableType::Hidden, true, None) => {
            Ok(VariableForm::Inline)
        }
        (VariableType::Auxiliary, false, None) => Ok(VariableForm::Repetition),
        (VariableType::Named, false, None) => Ok(VariableForm::Named),
        (VariableType::Anonymous, false, None) => Ok(VariableForm::Anonymous),
        (VariableType::Hidden, false, None) => Ok(VariableForm::Hidden),
    }
}

/// A node type as `node-types.json` names one.
#[derive(Serialize)]
struct NodeTypeEntry<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    named: bool,
}

#[derive(Serialize)]
struct ExternalTokenEntry<'a> {
    name: &'a str,
    kind: ExternalKind,
}

/// An external token's visibility. Tree-sitter derives it from the token's
/// spelling alone (`intern_symbols`: a name is named, or hidden when it starts
/// with `_`; a string is anonymous), so unlike a terminal it is never
/// auxiliary.
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ExternalKind {
    Named,
    Anonymous,
    Hidden,
}

/// The visibility of an external token of `kind`; none exactly for
/// `Auxiliary`, which Tree-sitter never gives an external token.
const fn external_kind(kind: VariableType) -> Option<ExternalKind> {
    match kind {
        VariableType::Named => Some(ExternalKind::Named),
        VariableType::Anonymous => Some(ExternalKind::Anonymous),
        VariableType::Hidden => Some(ExternalKind::Hidden),
        VariableType::Auxiliary => None,
    }
}

#[derive(Serialize)]
struct TerminalEntry<'a> {
    name: &'a str,
    kind: Kind,
    /// The token's source: a string, a pattern, or a token's body.
    rule: RuleEntry<'a>,
}

#[derive(Serialize)]
struct ProductionEntry<'a> {
    steps: Vec<StepEntry<'a>>,
    /// The production completes only at the end of input.
    requires_eof: bool,
}

#[derive(Serialize)]
struct StepEntry<'a> {
    symbol: SymbolRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<AliasEntry<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<&'a str>,
}

#[derive(Serialize, Clone, Copy)]
struct SymbolRef {
    kind: SymbolKind,
    index: u32,
}

#[derive(Serialize)]
struct AliasEntry<'a> {
    value: &'a str,
    named: bool,
}

#[derive(Serialize)]
struct DefaultAlias<'a> {
    symbol: SymbolRef,
    alias: AliasEntry<'a>,
}

/// A node of a rule tree.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RuleEntry<'a> {
    Blank,
    Eof,
    Symbol {
        symbol: SymbolRef,
    },
    String {
        value: &'a str,
    },
    Pattern {
        value: &'a str,
        flags: &'a str,
    },
    NamedSymbol {
        name: &'a str,
    },
    Seq {
        members: Vec<Self>,
    },
    Choice {
        members: Vec<Self>,
    },
    Repeat {
        content: Box<Self>,
    },
    Metadata {
        #[serde(skip_serializing_if = "Option::is_none")]
        alias: Option<AliasEntry<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        field: Option<&'a str>,
        is_token: bool,
        is_main_token: bool,
        content: Box<Self>,
    },
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Named,
    Anonymous,
    Hidden,
    Auxiliary,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum SymbolKind {
    Terminal,
    NonTerminal,
    External,
    End,
    EndOfNonTerminalExtra,
}

const fn kind(kind: VariableType) -> Kind {
    match kind {
        VariableType::Named => Kind::Named,
        VariableType::Anonymous => Kind::Anonymous,
        VariableType::Hidden => Kind::Hidden,
        VariableType::Auxiliary => Kind::Auxiliary,
    }
}

const fn symbol(symbol: Symbol) -> SymbolRef {
    SymbolRef {
        kind: match symbol.kind {
            SymbolType::Terminal => SymbolKind::Terminal,
            SymbolType::NonTerminal => SymbolKind::NonTerminal,
            SymbolType::External => SymbolKind::External,
            SymbolType::End => SymbolKind::End,
            SymbolType::EndOfNonTerminalExtra => SymbolKind::EndOfNonTerminalExtra,
        },
        index: symbol.index,
    }
}

fn alias(alias: Alias, pool: &StrPool) -> AliasEntry<'_> {
    AliasEntry {
        value: pool.resolve(alias.value),
        named: alias.is_named,
    }
}

fn rule<'a>(node: &StructuredRule, pool: &'a StrPool) -> RuleEntry<'a> {
    let all = |members: &[StructuredRule]| members.iter().map(|m| rule(m, pool)).collect();
    match node {
        StructuredRule::Blank => RuleEntry::Blank,
        StructuredRule::Eof => RuleEntry::Eof,
        StructuredRule::Symbol(s) => RuleEntry::Symbol { symbol: symbol(*s) },
        StructuredRule::String(value) => RuleEntry::String {
            value: pool.resolve(*value),
        },
        StructuredRule::Pattern(value, flags) => RuleEntry::Pattern {
            value: pool.resolve(*value),
            flags: pool.resolve(*flags),
        },
        StructuredRule::NamedSymbol(name) => RuleEntry::NamedSymbol {
            name: pool.resolve(*name),
        },
        StructuredRule::Seq(members) => RuleEntry::Seq {
            members: all(members),
        },
        StructuredRule::Choice(members) => RuleEntry::Choice {
            members: all(members),
        },
        StructuredRule::Repeat(content) => RuleEntry::Repeat {
            content: Box::new(rule(content, pool)),
        },
        StructuredRule::Metadata {
            alias: a,
            field,
            is_token,
            is_main_token,
            rule: content,
        } => RuleEntry::Metadata {
            alias: a.map(|a| alias(a, pool)),
            field: field.map(|f| pool.resolve(f)),
            is_token: *is_token,
            is_main_token: *is_main_token,
            content: Box::new(rule(content, pool)),
        },
    }
}

/// A repetition's repeated element: its tree is a choice of one plain
/// `[self, self]` sequence and the elements (`wrap_in_binary_tree`, which
/// leaves the sequence bare when there are no elements). An element that
/// matches nothing is an empty choice.
fn repeated_element(tree: &StructuredRule, symbol: Symbol) -> Option<StructuredRule> {
    let binary = StructuredRule::Seq(vec![
        StructuredRule::Symbol(symbol),
        StructuredRule::Symbol(symbol),
    ]);
    let members = match tree {
        StructuredRule::Choice(members) => members.as_slice(),
        bare => std::slice::from_ref(bare),
    };
    if members.iter().filter(|m| **m == binary).count() != 1 {
        return None;
    }
    let mut elements = members
        .iter()
        .filter(|m| **m != binary)
        .cloned()
        .collect::<Vec<_>>();
    if elements.len() == 1 {
        elements.pop()
    } else {
        Some(StructuredRule::Choice(elements))
    }
}

/// Whether a production is a repetition's plain `[self, self]` production.
fn is_binary(key: &ProductionKey, symbol: Symbol) -> bool {
    !key.requires_eof
        && key.steps.len() == 2
        && key
            .steps
            .iter()
            .all(|step| step.symbol == symbol && step.alias.is_none() && step.field.is_none())
}

/// Productions, each once, in the order first reached.
type ProductionSet = IndexSet<ProductionKey, BuildHasherDefault<FxHasher>>;

fn production<'a>(key: &ProductionKey, pool: &'a StrPool) -> ProductionEntry<'a> {
    ProductionEntry {
        steps: key
            .steps
            .iter()
            .map(|step| StepEntry {
                symbol: symbol(step.symbol),
                alias: step.alias.map(|a| alias(a, pool)),
                field: step.field.map(|f| pool.resolve(f)),
            })
            .collect(),
        requires_eof: key.requires_eof,
    }
}

/// The grammar contract of `input_grammar`, as JSON.
///
/// The grammar is prepared afresh, capturing the rule trees on the way; the
/// diagnostics of that preparation repeat those of the parser's own and are
/// not reported twice.
///
/// # Errors
///
/// [`ContractError`] if the grammar cannot be prepared, a rule tree does not
/// flatten to its variable's productions, an internal invariant of the
/// prepared grammar fails, or serialization fails.
pub fn generate_grammar_contract_json(
    input_grammar: InputGrammar,
) -> Result<String, ContractError> {
    let mut repeated_diagnostics = Vec::new();
    let name = input_grammar.pool.resolve(input_grammar.name).to_string();
    let (prepared, structure) =
        prepare_grammar_with_structure(input_grammar, &mut repeated_diagnostics)?;
    contract_json(&name, &prepared, &structure?)
}

/// The contract of the grammar `name`, prepared, whose rule trees are
/// `structure`.
fn contract_json(
    name: &str,
    prepared: &PreparedGrammar,
    structure: &ContractStructure,
) -> Result<String, ContractError> {
    let variable_info = get_variable_info(
        &prepared.syntax_grammar,
        &prepared.lexical_grammar,
        &prepared.default_aliases,
        &prepared.str_pool,
    )?;
    let writer = Writer {
        prepared,
        variable_info: &variable_info,
    };
    let syntax_grammar = &prepared.syntax_grammar;
    let pool = &prepared.str_pool;
    let contract = Contract {
        format: CONTRACT_FORMAT,
        name,
        variables: structure
            .variables
            .iter()
            .enumerate()
            .map(|(index, variable)| writer.variable(index, variable))
            .collect::<Result<_, _>>()?,
        terminals: writer.terminals(structure)?,
        external_tokens: syntax_grammar
            .external_tokens
            .iter()
            .map(|t| {
                let name = pool.resolve(t.name);
                external_kind(t.kind)
                    .map(|kind| ExternalTokenEntry { name, kind })
                    .ok_or_else(|| ContractError::AuxiliaryExternal(name.to_string()))
            })
            .collect::<Result<_, _>>()?,
        extras: syntax_grammar
            .extra_symbols
            .iter()
            .copied()
            .map(symbol)
            .collect(),
        word_token: syntax_grammar.word_token.map(symbol),
        default_aliases: prepared
            .default_aliases
            .iter()
            .map(|(&s, &a)| DefaultAlias {
                symbol: symbol(s),
                alias: alias(a, pool),
            })
            .collect(),
        node_kinds: node_kinds(
            syntax_grammar,
            &prepared.lexical_grammar,
            &prepared.default_aliases,
            &variable_info,
            pool,
        )?
        .into_iter()
        .map(|(kind, named)| NodeTypeEntry { kind, named })
        .collect(),
    };
    serde_json::to_string(&contract).map_err(|e| ContractError::Serialize(e.to_string()))
}

/// A prepared grammar, with the child information its subtypes come from.
struct Writer<'a> {
    prepared: &'a PreparedGrammar,
    variable_info: &'a [VariableInfo],
}

impl<'a> Writer<'a> {
    /// The final productions of variable `index`, each once, in the order
    /// inlining produces them, with every step's effective alias (its own, or
    /// else its symbol's default alias, which `extract_default_aliases`
    /// removed from the step).
    fn final_productions(&self, index: usize) -> Result<ProductionSet, MissingInlineExpansion> {
        let syntax_grammar = &self.prepared.syntax_grammar;
        let default_aliases = &self.prepared.default_aliases;
        let mut keys = ProductionSet::default();
        for prod_id in syntax_grammar.variable_prod_ids(index) {
            for id in final_productions(syntax_grammar, &self.prepared.inlines, prod_id)? {
                let production = syntax_grammar.production(id);
                keys.insert(ProductionKey {
                    steps: production
                        .steps
                        .iter()
                        .map(|s| StepKey {
                            symbol: s.symbol(),
                            alias: s
                                .alias()
                                .or_else(|| default_aliases.get(&s.symbol()).copied()),
                            field: s.field(),
                        })
                        .collect(),
                    requires_eof: production.requires_eof_lookahead,
                });
            }
        }
        Ok(keys)
    }

    /// Variable `index`'s entry, its rule tree proven against its productions.
    fn variable(
        &self,
        index: usize,
        structure: &VariableStructure,
    ) -> Result<VariableEntry<'a>, ContractError> {
        let syntax_grammar = &self.prepared.syntax_grammar;
        let pool = &self.prepared.str_pool;
        let variable = &syntax_grammar.variables[index];
        let name = || pool.resolve(variable.name).to_string();
        let as_symbol = Symbol::non_terminal(index);
        let mut keys = self.final_productions(index)?;

        let paths = &structure.paths;
        let tree_only = paths.iter().filter(|p| !keys.contains(*p)).count();
        let productions_only = keys.iter().filter(|k| !paths.contains(*k)).count();
        if tree_only + productions_only > 0 {
            return Err(ContractError::Unproven {
                variable: name(),
                tree_only,
                productions_only,
            });
        }

        let subtypes = syntax_grammar
            .supertype_symbols
            .contains(&as_symbol)
            .then(|| {
                supertype_subtypes(
                    &self.variable_info[index],
                    syntax_grammar,
                    &self.prepared.lexical_grammar,
                    &self.prepared.default_aliases,
                    pool,
                )
                .into_iter()
                .map(|(kind, named)| NodeTypeEntry { kind, named })
                .collect()
            });
        let form = variable_form(
            variable.kind,
            syntax_grammar.variables_to_inline.contains(&as_symbol),
            subtypes,
        )
        .map_err(|form| ContractError::VariableForm {
            variable: name(),
            form,
        })?;
        let rule_entry = match form {
            // The tree has exactly one binary sequence, and the proof put its
            // path among the productions; it is stated by the form instead.
            VariableForm::Repetition => {
                let element = repeated_element(&structure.rule, as_symbol)
                    .ok_or_else(|| ContractError::AuxiliaryStructure(name()))?;
                keys.retain(|k| !is_binary(k, as_symbol));
                rule(&element, pool)
            }
            VariableForm::Named
            | VariableForm::Anonymous
            | VariableForm::Hidden
            | VariableForm::Supertype { .. }
            | VariableForm::Inline => rule(&structure.rule, pool),
        };
        Ok(VariableEntry {
            name: pool.resolve(variable.name),
            form,
            rule: rule_entry,
            productions: keys.iter().map(|k| production(k, pool)).collect(),
        })
    }

    /// The terminals with their source trees. The trees are in extraction
    /// order; the lexical grammar is built in the same order, which the names
    /// confirm rather than assume.
    fn terminals(
        &self,
        structure: &ContractStructure,
    ) -> Result<Vec<TerminalEntry<'a>>, ContractError> {
        let lexical_grammar = &self.prepared.lexical_grammar;
        let pool = &self.prepared.str_pool;
        let lexical = lexical_grammar.variables.len();
        let sources = structure.terminals.len();
        if let Some(index) = (0..lexical.max(sources)).find(|&i| {
            match (lexical_grammar.variables.get(i), structure.terminals.get(i)) {
                (Some(v), Some(source)) => v.name != source.name,
                _ => true,
            }
        }) {
            return Err(ContractError::TerminalSources {
                lexical,
                sources,
                index,
            });
        }
        Ok(lexical_grammar
            .variables
            .iter()
            .zip(&structure.terminals)
            .map(|(v, source)| TerminalEntry {
                name: pool.resolve(v.name),
                kind: kind(v.kind),
                rule: rule(&source.rule, pool),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        ContractError, IllegalForm, NodeTypeEntry, contract_json, external_kind,
        generate_grammar_contract_json, variable_form,
    };
    use crate::node_types::{generate_node_types_json, get_variable_info};
    use crate::{
        grammars::VariableType, parse_grammar::parse_grammar,
        prepare_grammar::prepare_grammar_with_structure,
    };

    fn contract(grammar: &Value) -> Value {
        let mut diagnostics = Vec::new();
        let input = parse_grammar(&grammar.to_string(), &mut diagnostics).unwrap();
        let json = generate_grammar_contract_json(input).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    fn variable<'a>(contract: &'a Value, name: &str) -> &'a Value {
        contract["variables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == name)
            .unwrap_or_else(|| panic!("no variable `{name}`"))
    }

    fn productions(variable: &Value) -> &Vec<Value> {
        variable["productions"].as_array().unwrap()
    }

    #[test]
    fn test_repetitions_inlining_aliases_and_fields_are_stated_not_left_to_inference() {
        let grammar = json!({
            "name": "contract_probe",
            "rules": {
                "source": { "type": "SEQ", "members": [
                    { "type": "REPEAT", "content": { "type": "SYMBOL", "name": "item" } },
                    { "type": "SYMBOL", "name": "_run" },
                    { "type": "SYMBOL", "name": "inlined" },
                    { "type": "ALIAS", "value": "renamed", "named": true,
                      "content": { "type": "SYMBOL", "name": "item" } },
                    { "type": "FIELD", "name": "tail", "content": { "type": "SYMBOL", "name": "_value" } }
                ]},
                "item": { "type": "STRING", "value": "i" },
                "_run": { "type": "REPEAT1", "content": { "type": "STRING", "value": "r" } },
                "inlined": { "type": "SEQ", "members": [
                    { "type": "STRING", "value": "(" }, { "type": "STRING", "value": ")" }
                ]},
                "_value": { "type": "CHOICE", "members": [
                    { "type": "SYMBOL", "name": "item" }, { "type": "SYMBOL", "name": "word" }
                ]},
                "word": { "type": "PATTERN", "value": "[a-z]+" }
            },
            "extras": [{ "type": "PATTERN", "value": "\\s" }],
            "inline": ["inlined"],
            "supertypes": ["_value"]
        });
        let contract = contract(&grammar);
        assert_eq!(contract["format"], super::CONTRACT_FORMAT);
        assert_eq!(contract["name"], "contract_probe");

        // `source` has two productions, because `repeat` is zero or more: one
        // with the auxiliary repeat step and one without. In the first, the
        // inline rule's two literals are spliced in and the alias and field
        // ride on their steps.
        let source = variable(&contract, "source");
        let lengths = productions(source)
            .iter()
            .map(|p| p["steps"].as_array().unwrap().len())
            .collect::<Vec<_>>();
        assert_eq!(lengths, [6, 5], "{source:?}");
        let steps = productions(source)[0]["steps"].as_array().unwrap();
        let renamed = json!({ "value": "renamed", "named": true });
        assert_eq!(
            steps.iter().filter(|step| step["alias"] == renamed).count(),
            1,
            "{steps:?}"
        );
        assert_eq!(steps.last().unwrap()["field"], "tail", "{steps:?}");

        // The repeat's auxiliary variable is stated as a repetition of `item`,
        // without its binary production.
        let aux_index = steps[0]["symbol"]["index"].as_u64().unwrap() as usize;
        let aux = &contract["variables"][aux_index];
        assert_eq!(aux["form"], "repetition");
        assert_eq!(productions(aux).len(), 1);

        // A hidden rule whose body is a top-level repeat becomes a repetition
        // itself, which is never inline.
        assert_eq!(variable(&contract, "_run")["form"], "repetition");

        // Subtypes are present exactly for a supertype.
        let value = variable(&contract, "_value");
        assert_eq!(value["form"], "supertype");
        assert_eq!(value["subtypes"].as_array().unwrap().len(), 2, "{value:?}");
        assert!(source.get("subtypes").is_none());
        assert_eq!(variable(&contract, "inlined")["form"], "inline");
    }

    #[test]
    fn test_a_default_alias_is_stated_on_every_step_it_renames() {
        // `b` always appears aliased as `x`, so it becomes a default alias and
        // `extract_default_aliases` removes the alias from the steps. The
        // contract restores it on each, and the proof compares with it.
        let grammar = json!({
            "name": "default_alias",
            "rules": {
                "source": { "type": "SEQ", "members": [
                    { "type": "ALIAS", "value": "x", "named": true,
                      "content": { "type": "SYMBOL", "name": "b" } },
                    { "type": "STRING", "value": ";" }
                ]},
                "b": { "type": "STRING", "value": "b" }
            }
        });
        let contract = contract(&grammar);
        assert_eq!(contract["default_aliases"].as_array().unwrap().len(), 1);
        let steps = &productions(variable(&contract, "source"))[0]["steps"];
        assert_eq!(steps[0]["alias"], json!({ "value": "x", "named": true }));
    }

    #[test]
    fn test_alternatives_that_flatten_alike_are_one_production() {
        // The two alternatives differ only in precedence, which does not
        // shape a tree: one production, and a tree without the precedence.
        let grammar = json!({
            "name": "duplicate",
            "rules": {
                "source": { "type": "CHOICE", "members": [
                    { "type": "PREC", "value": 1, "content": { "type": "SYMBOL", "name": "a" } },
                    { "type": "PREC", "value": 2, "content": { "type": "SYMBOL", "name": "a" } }
                ]},
                "a": { "type": "STRING", "value": "a" }
            }
        });
        let contract = contract(&grammar);
        let source = variable(&contract, "source");
        assert_eq!(productions(source).len(), 1, "{source:?}");
        assert_eq!(
            source["rule"]["members"].as_array().unwrap().len(),
            1,
            "{source:?}"
        );
    }

    #[test]
    fn test_a_repetition_whose_element_matches_nothing_has_an_empty_element() {
        let grammar = json!({
            "name": "empty_repetition",
            "rules": {
                "source": { "type": "SEQ", "members": [
                    { "type": "STRING", "value": "a" },
                    { "type": "REPEAT", "content": { "type": "CHOICE", "members": [] } }
                ]}
            }
        });
        let contract = contract(&grammar);
        let aux = contract["variables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["form"] == "repetition")
            .unwrap();
        assert_eq!(aux["rule"], json!({ "type": "choice", "members": [] }));
        assert_eq!(productions(aux).len(), 0, "{aux:?}");
    }

    #[test]
    fn test_inlined_end_of_input_is_proven() {
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
        let contract = contract(&grammar);
        let source = productions(variable(&contract, "source"));
        assert_eq!(source.len(), 2, "{source:?}");
        assert!(source.iter().all(|p| p["requires_eof"] == true));
    }

    #[test]
    fn test_an_eof_followed_in_its_own_rule_by_an_empty_inline_is_refused() {
        // Flattening discards `seq("a", eof, _opt)` outright, because `eof` is
        // not the rule's own last step, even where `_opt` is later inlined as
        // nothing. The tree reads `a, eof` as a live path by the end-of-input
        // rule of the spliced tree, which cannot see the rule boundary, so the
        // proof refuses the contract rather than state a production the parser
        // does not have.
        let grammar = json!({
            "name": "eof_then_empty_inline",
            "rules": {
                "source": { "type": "CHOICE", "members": [
                    { "type": "SEQ", "members": [
                        { "type": "STRING", "value": "a" },
                        { "type": "EOF" },
                        { "type": "SYMBOL", "name": "_opt" }
                    ]},
                    { "type": "STRING", "value": "c" }
                ]},
                "_opt": { "type": "CHOICE", "members": [
                    { "type": "STRING", "value": "b" }, { "type": "BLANK" }
                ]}
            },
            "inline": ["_opt"]
        });
        let mut diagnostics = Vec::new();
        let input = parse_grammar(&grammar.to_string(), &mut diagnostics).unwrap();
        let refused = generate_grammar_contract_json(input);
        assert!(
            matches!(
                refused,
                Err(ContractError::Unproven {
                    tree_only: 1,
                    productions_only: 0,
                    ..
                })
            ),
            "{refused:?}"
        );
    }

    /// The (type, named) pairs `node-types.json` lists for `grammar`, in its
    /// order, from the writer `generate` uses.
    fn listed_node_types(grammar: &Value) -> Vec<Value> {
        let mut diagnostics = Vec::new();
        let input = parse_grammar(&grammar.to_string(), &mut diagnostics).unwrap();
        let (prepared, _) = prepare_grammar_with_structure(input, &mut diagnostics).unwrap();
        let info = get_variable_info(
            &prepared.syntax_grammar,
            &prepared.lexical_grammar,
            &prepared.default_aliases,
            &prepared.str_pool,
        )
        .unwrap();
        let json = generate_node_types_json(
            &prepared.syntax_grammar,
            &prepared.lexical_grammar,
            &prepared.default_aliases,
            &info,
            &prepared.str_pool,
        )
        .unwrap();
        serde_json::from_str::<Vec<Value>>(&json)
            .unwrap()
            .iter()
            .map(|node| json!({ "type": node["type"], "named": node["named"] }))
            .collect()
    }

    #[test]
    fn test_node_kinds_are_the_kinds_node_types_lists() {
        // `inner` is renamed at its only reference, so it gets a default alias
        // and no node carries its own name: listed as `outer`, never `inner`.
        let grammar = json!({
            "name": "renamed",
            "rules": {
                "source": { "type": "SEQ", "members": [
                    { "type": "ALIAS", "value": "outer", "named": true,
                      "content": { "type": "SYMBOL", "name": "inner" } },
                    { "type": "SYMBOL", "name": "_value" }
                ]},
                "inner": { "type": "SEQ", "members": [
                    { "type": "STRING", "value": "a" }, { "type": "STRING", "value": "b" }
                ]},
                "_value": { "type": "CHOICE", "members": [
                    { "type": "SYMBOL", "name": "number" }, { "type": "SYMBOL", "name": "word" }
                ]},
                "number": { "type": "PATTERN", "value": "\\d+" },
                "word": { "type": "PATTERN", "value": "[a-z]+" }
            },
            "supertypes": ["_value"]
        });
        let kinds = contract(&grammar)["node_kinds"].as_array().unwrap().clone();
        assert_eq!(kinds, listed_node_types(&grammar));
        let names: Vec<&str> = kinds.iter().filter_map(|k| k["type"].as_str()).collect();
        assert!(
            names.contains(&"outer") && !names.contains(&"inner"),
            "{names:?}"
        );
        assert!(names.contains(&"_value"), "{names:?}");
    }

    #[test]
    fn test_combinations_tree_sitter_does_not_build_have_no_form() {
        // The forms Tree-sitter does build are checked end to end, through
        // the contract, by the first test.
        let subtypes = || Some(Vec::<NodeTypeEntry>::new());
        assert!(matches!(
            variable_form(VariableType::Hidden, true, subtypes()),
            Err(IllegalForm::InlineSupertype)
        ));
        assert!(matches!(
            variable_form(VariableType::Named, false, subtypes()),
            Err(IllegalForm::VisibleSupertype)
        ));
        assert!(matches!(
            variable_form(VariableType::Anonymous, false, subtypes()),
            Err(IllegalForm::VisibleSupertype)
        ));
        assert!(matches!(
            variable_form(VariableType::Auxiliary, true, None),
            Err(IllegalForm::InlineRepetition)
        ));
        assert!(matches!(
            variable_form(VariableType::Auxiliary, false, subtypes()),
            Err(IllegalForm::RepetitionSupertype)
        ));
    }

    #[test]
    fn test_an_external_token_is_never_auxiliary() {
        assert!(external_kind(VariableType::Hidden).is_some());
        assert!(external_kind(VariableType::Auxiliary).is_none());
    }

    #[test]
    fn test_a_tree_that_does_not_flatten_to_the_productions_is_refused() {
        // The negative control: the same preparation, with one path removed
        // from the tree's, must be refused by the proof.
        let grammar = json!({
            "name": "tampered",
            "rules": {
                "source": { "type": "CHOICE", "members": [
                    { "type": "STRING", "value": "a" }, { "type": "STRING", "value": "b" }
                ]}
            }
        });
        let mut diagnostics = Vec::new();
        let input = parse_grammar(&grammar.to_string(), &mut diagnostics).unwrap();
        let (prepared, structure) =
            prepare_grammar_with_structure(input, &mut diagnostics).unwrap();
        let mut structure = structure.unwrap();
        let paths = &mut structure.variables[0].paths;
        let removed = paths.iter().next().cloned().unwrap();
        paths.remove(&removed);
        let refused = contract_json("tampered", &prepared, &structure);
        assert!(
            matches!(
                refused,
                Err(ContractError::Unproven {
                    tree_only: 0,
                    productions_only: 1,
                    ..
                })
            ),
            "{refused:?}"
        );
    }
}
