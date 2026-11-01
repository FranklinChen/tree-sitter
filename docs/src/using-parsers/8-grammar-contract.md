# The Grammar Contract

```admonish info
The grammar contract is an addition of this fork of Tree-sitter, not a feature of upstream Tree-sitter, and has not been
proposed there. See `FORK.md` at the root of the repository for its scope and how the fork is maintained.
```

`node-types.json` tells you which kinds of node may appear beneath each kind of node. It does not tell you in what order
they appear, through which grammar production, or which alias and field each position carries. A tool that needs those
facts, such as a generator of typed tree traversals, has had to re-derive them from `grammar.json` by re-implementing the
parts of `tree-sitter generate` that decide them: token extraction, repeat expansion, inlining and alias application.
Every such re-implementation can drift from the real one.

`tree-sitter generate --grammar-contract` therefore also writes `src/grammar-contract.json`: the facts the generator
itself computes, taken from the same preparation of the grammar that `node-types.json` and the parse tables are built
from. A consumer reads them instead of inferring them. Without the flag, `generate` writes exactly what upstream
Tree-sitter writes.

## Guarantees

- **It is checked by the generator.** Each variable's `rule` (below) is a tree whose paths the generator enumerates with
  its own flattening code; they must be exactly the variable's final `productions`, step for step (symbol, alias, field)
  and in whether the production requires the end of input. If any variable differs, `generate` fails instead of writing
  the file.
- **It changes nothing else.** The contract is prepared separately, after the parser and node types, and before any file
  is written. `parser.c`, `node-types.json`, `grammar.json` and the headers are byte-identical with and without the flag,
  and a grammar upstream refuses is refused with the same message.
- **Symbols are numbers, not names.** Every symbol is written as `{ "kind": ..., "index": ... }`, an index into the array
  its kind names, exactly as the generator numbers it. No consumer has to match names.
- **It is versioned.** `format` names the format and its version. A published version is never changed; any change a
  consumer must know about is a new version. This page describes `tree-sitter-contract-fork/grammar-contract/5`, whose
  JSON schema is `docs/src/assets/schemas/grammar-contract.schema.json`.

## Top level

```json
{
  "format": "tree-sitter-contract-fork/grammar-contract/5",
  "name": "json",
  "builtin_symbol_names": ["end"],
  "variables": [ ... ],
  "terminals": [ ... ],
  "external_tokens": [ ... ],
  "extras": [ ... ],
  "word_token": null,
  "default_aliases": [ ... ],
  "node_kinds": [ ... ]
}
```

`builtin_symbol_names` lists runtime-reserved names from the renderer's string pool.
Consumers checking name uniqueness must include these in addition to grammar symbols
and aliases. Version 5 also corrects the kind inventory for a default-aliased start
variable: its root uses the alias, just as nested occurrences do. This changes the
metadata inventory, not the generated parser's behavior.

`node_kinds` includes both definitions and references in `node-types.json`.
An inline alias can appear only in a child or field reference, so top-level
entries alone are not a complete kind inventory. Definition order is retained;
reference-only kinds follow in spelling order.

- `name`: the grammar's name, as `grammar.json` states it and the parser's `tree_sitter_<name>` function is named.
- `variables`: the syntax variables (`"kind": "non_terminal"` symbols), in the generator's order.
- `terminals`: the lexical variables (`"kind": "terminal"` symbols), in the generator's order.
- `external_tokens`: the external tokens (`"kind": "external"` symbols), in the grammar's order. Each has `name` and
  `kind`.
- `extras`: symbols.
- `word_token`: the keyword-extraction token, or `null`.
- `default_aliases`: `{ "symbol": ..., "alias": { "value": ..., "named": ... } }` for every symbol that never appears
  unaliased in a production. The runtime applies such an alias everywhere the symbol appears, including inside `ERROR`
  nodes; in the contract's own productions and rule trees it is already written on each step.
- `node_kinds`: every defined or referenced kind in `node-types.json`, named and anonymous, as `{ "type", "named" }`.
  Definitions retain writer order and reference-only kinds follow in spelling order. It answers which kinds exist, which the
  rest of the contract states only indirectly: a visible rule or token that every reference renames (one with a default
  alias) is never a node of its own name, so it is listed only under its alias.

A terminal's or external token's `kind` is its visibility: `named` and `anonymous` tokens are nodes; `hidden` ones are
not, and neither are `auxiliary` terminals, which are tokens Tree-sitter extracted from a rule. An external token is
never `auxiliary`: Tree-sitter derives its visibility from its spelling alone. A variable states its `form` instead
(below).

The file is written without whitespace; a JSON pretty-printer makes it readable.

## Variables

```json
{
  "name": "pair",
  "form": "named",
  "rule": { ... },
  "productions": [ { "steps": [ ... ], "requires_eof": false } ]
}
```

- `form`: what the variable is. Tree-sitter keeps a variable's kind, whether it is inlined and whether it is a supertype
  as separate facts, but builds only six combinations of them, and `form` names the combination:
  - `named`, `anonymous`: a visible node, named or anonymous.
  - `hidden`: no node; its children are placed in its parent.
  - `supertype`: hidden and listed in the grammar's `supertypes`. Only this form has `subtypes`: its subtypes as
    `node-types.json` lists them, `{ "type", "named" }` in the same order, computed by the same code.
  - `inline`: listed in the grammar's `inline`, so it is replaced by its productions wherever it is referenced and never
    produces a node.
  - `repetition`: a variable Tree-sitter generated for a repeat (below).

- `rule`: the variable's rule tree just before flattening, with tokens extracted, repeats replaced by references to
  their auxiliary variables, and every reference to an `inline` variable replaced by that variable's tree.
- `productions`: the variable's final productions after inlining, each written once. Each step has a `symbol` and, when
  present, an `alias` (`{ "value", "named" }`) and a `field`; `requires_eof` is true when the production completes only
  at the end of input.
Every auxiliary variable Tree-sitter generates is a `repetition`, and so is a hidden rule whose body is a top-level
repeat (which also stops being inlined). For a repetition, `rule` and `productions` describe one element: the variable stands for one or more of
them, joined by invisible binary subtrees. The generator checks that the variable has exactly one binary
`[self, self]` production before leaving it out. An element that matches nothing (a repeat of an empty `choice`) is
written as an empty `choice`, with no productions.

For example, from the JSON grammar, where `pair` is `seq(field('key', $._key), ':', field('value', $._value))`:

```json
{
  "name": "pair",
  "form": "named",
  "rule": { "type": "seq", "members": [
    { "type": "metadata", "field": "key", "is_token": false, "is_main_token": false,
      "content": { "type": "symbol", "symbol": { "kind": "non_terminal", "index": 5 } } },
    { "type": "symbol", "symbol": { "kind": "terminal", "index": 3 } },
    { "type": "metadata", "field": "value", "is_token": false, "is_main_token": false,
      "content": { "type": "symbol", "symbol": { "kind": "non_terminal", "index": 1 } } }
  ] },
  "productions": [ { "steps": [
    { "symbol": { "kind": "non_terminal", "index": 5 }, "field": "key" },
    { "symbol": { "kind": "terminal", "index": 3 } },
    { "symbol": { "kind": "non_terminal", "index": 1 }, "field": "value" }
  ], "requires_eof": false } ]
}
```

and the repetition behind `object`'s `commaSep(...)`, whose element is a comma followed by a pair:

```json
{
  "name": "object_repeat1",
  "form": "repetition",
  "rule": { "type": "seq", "members": [
    { "type": "symbol", "symbol": { "kind": "terminal", "index": 1 } },
    { "type": "symbol", "symbol": { "kind": "non_terminal", "index": 3 } }
  ] },
  "productions": [ { "steps": [
    { "symbol": { "kind": "terminal", "index": 1 } },
    { "symbol": { "kind": "non_terminal", "index": 3 } }
  ], "requires_eof": false } ]
}
```

### Reading a rule tree

A tree has the node types of `grammar.json` with symbols resolved: `blank`, `symbol`, `seq`, `choice`, `metadata`
(carrying `alias`, `field`, `is_token` and `is_main_token`), and, where they survive preparation, `eof`, `string`,
`pattern` and `repeat`. Read it as the generator flattens it:

- An alias or field applies to every step beneath it, and an inner one overrides an outer one. Where an inline variable's
  tree is spliced in under an alias (field), the tree's own aliases (fields) are already removed, because inlining gives
  the reference's precedence over the body's.
- A path is a production when every `eof` on it comes after all its other steps; those `eof`s together mean the
  production completes only at the end of input. A path with a step after an `eof` is not a production.
- Alternatives that match nothing (through an empty `choice`) are already removed, so a `choice` is empty only where the
  whole tree, or a repetition's element, matches nothing.
- `repeat(x)` reaches the tree as `choice(<x_repeat>, blank)`, where `<x_repeat>` is a repetition variable. That is also
  exactly how `optional(repeat1(x))` reaches it: the two are one rule to Tree-sitter.
- Nested choices are already flattened and their duplicate alternatives removed, so `choice(optional(x), y)` and
  `optional(choice(x, y))` reach the tree identically.

Whether a step is a node is the runtime's rule: it is a node when it carries an alias, or when its symbol is `named` or
`anonymous`. A step on a hidden variable places that variable's steps in the parent; a step on a hidden token is text with
no node.

One grammar shape is refused rather than written: an `eof` followed, within its own rule, by a reference to an inline
variable that can match nothing. Tree-sitter discards that alternative because the `eof` is not its rule's last step,
which the spliced tree, having no rule boundaries, cannot show; the check then fails, and `generate --grammar-contract`
reports the variable.

## Terminals

```json
{ "name": ":", "kind": "anonymous", "rule": { "type": "string", "value": ":" } }
```

Each terminal's `rule` is its source as token extraction keeps it. Extraction drops a `token(...)` wrapper that carries
nothing else and merges tokens with equal sources, so `token(/x/)` and `/x/` are one terminal whose `rule` is the
pattern. Other metadata is kept as `metadata` (`is_token`, `is_main_token` for `token.immediate`); precedence is not
written. Terminal names are checked against the lexical grammar's, index by index.

## Not included

Precedence, associativity, dynamic precedence, conflicts, reserved-word contexts and the per-variable child summaries
`node-types.json` is computed from are not written. The contract describes the structure of trees, not how the parser
chooses between them or lexes their tokens.
