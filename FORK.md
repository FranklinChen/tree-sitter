# This fork: the grammar contract

This branch, `contract/v0.27.1`, is Tree-sitter at tag `v0.27.1` plus one opt-in feature: `tree-sitter generate
--grammar-contract` also writes `src/grammar-contract.json`, a machine-readable statement of the prepared grammar the
parser is generated from. Without the flag, the fork behaves exactly as `v0.27.1`. The feature is not part of upstream
Tree-sitter, and there is no plan to propose it there. It is kept small and in upstream's style anyway, because that is
what keeps it cheap to carry from one upstream release to the next.

The format, and how to read it, is documented in the book:
[The Grammar Contract](docs/src/using-parsers/8-grammar-contract.md), with a JSON schema at
[`docs/src/assets/schemas/grammar-contract.schema.json`](docs/src/assets/schemas/grammar-contract.schema.json).

## Why

A typed traversal generator needs to know, for every node kind, which children appear in what order, through which
production, with which alias and field. `node-types.json` does not say. The only way to know was to re-derive it from
`grammar.json`, re-implementing token extraction, repeat expansion, inlining and alias application, and every
re-implementation drifted from Tree-sitter's in ways no test written by the same author could see. The facts belong to
the generator, which already computes them; the contract publishes them.

## Scope

What the fork changes, and nothing more:

- **One CLI flag.** `generate --grammar-contract` writes `src/grammar-contract.json`, with or without `--no-parser`.
- **One library parameter.** `tree_sitter_generate::generate_parser_in_directory` takes a `GrammarContract`
  (`Omit` or `Write`) before its diagnostics. New public items: `GrammarContract`, `ContractError` and the
  `GenerateError::Contract` variant. No other public signature changes.
- **A separate preparation.** With the flag, the grammar is prepared a second time, after the parser and node types are
  generated and before any file is written, capturing the rule trees on the way. The contract's own failure is reported
  as an error; it cannot change the parser, and a grammar upstream refuses is refused first, with upstream's message.
- **Shared code, refactored without change of behavior.** Three upstream functions are split so the contract uses the
  same code the parser does: flattening's per-variable path enumeration (`flatten_variable`), inlining's walk to a
  production's final productions (`walk_inlined_productions`, which allocates nothing more than upstream's loop did),
  and the subtype list of
  `node-types.json` (`supertype_subtype_refs`). `prepare_grammar` gains a capturing variant used only by the contract.

What it does not change: the runtime library, the Wasm bindings, every byte of `parser.c`, `node-types.json`,
`grammar.json` and the headers (with or without the flag), every error message, and every other command.

Files: `crates/generate/src/contract.rs` (the writer and its proof), `crates/generate/src/prepare_grammar/contract_structure.rs`
(the rule trees and their paths), `flatten_grammar.rs`, `process_inlines.rs`, `node_types.rs`, `prepare_grammar.rs` and
`generate.rs` (the shared code and wiring), `crates/cli/src/main.rs` (the flag), the book pages above,
`docs/src/cli/generate.md`, this file, `fork/acceptance.sh` and `.github/workflows/contract.yml`.

## How the contract is proven

Each variable's rule tree is the tree just before flattening, with every inline reference replaced by the inline
variable's tree (the reference's alias and field winning, as inlining applies them), alternatives that match nothing
pruned, and reserved-word contexts and precedence dropped. The generator enumerates the tree's paths with its own
flattening code and compares them with the variable's final productions after inlining, step by step (symbol, alias with
default aliases restored, field) and in the end-of-input requirement. Any difference refuses the contract.

One shape is refused by design: an `eof()` followed, within its own rule, by an inline reference that may match nothing.
Flattening drops that path because the `eof()` is not its rule's last step, which the spliced tree cannot show. No
grammar among Tree-sitter's fixtures has this shape.

## Commits

On top of `v0.27.1`, seven commits, by subject:

1. `refactor(generate): factor out flattening's path enumeration, inlining's walk and supertype subtypes`: no change of
   behavior or output; three upstream loops become functions the contract can share. A conflict here, when adopting a
   new upstream release, means upstream changed the code the contract depends on.
2. ``feat(generate): write the grammar contract with `generate --grammar-contract` ``: the writer, the rule trees and
   their proof, the flag, the book page, the CLI page and the JSON schema.
3. `ci: check the fork's generator, Tree-sitter's suite and acceptance against upstream`: the workflow and
   `fork/acceptance.sh`.
4. `docs: record the fork's scope and how it follows upstream releases`: this file.
5. `ci: normalize node's process id in acceptance refusals, and show differences`: Node prefixes its warnings with
   `(node:<pid>)`, which differs between runs, so acceptance compares refusals with it removed and prints any remaining
   difference.
6. `feat(generate)!: state each variable's form as one value (contract format 2)`: format 1 wrote a variable's kind,
   inlining and subtypes as three fields, which admitted combinations Tree-sitter never builds; format 2 writes the
   combination as one `form`, and an external token's `kind` without the `auxiliary` Tree-sitter never gives one.
7. `ci: compare acceptance refusals without backtraces`: CI's Rust setup sets `RUST_BACKTRACE`, and a backtrace lists
   the frames of the binary that refused, which differ between the fork's debug build and upstream's release build;
   acceptance now runs every CLI with backtraces off.

## How it has been verified

As of 2026-10-30, on this series:

- **Generator tests and lints.** `cargo test -p tree-sitter-generate --lib` (93 tests, among them the proof's negative
  control, which removes one path from a tree and expects the refusal, and regression tests for every defect the reviews
  found), `cargo clippy -p tree-sitter-generate --all-targets -- -D warnings`, `cargo clippy -p tree-sitter-cli -- -D
  warnings`, and a build with `--no-default-features`.
- **Acceptance against upstream.** `fork/acceptance.sh` over all 17 fetched fixture grammars (each `src/grammar.json` as
  shipped) and every test grammar in `test/fixtures/test_grammars` (converted from `grammar.js` by upstream's CLI):
  - 77 grammars: upstream `v0.27.1`, the fork, and the fork with `--grammar-contract` write byte-identical `parser.c`,
    `node-types.json`, `grammar.json` and `tree_sitter/` headers; the plain run writes no contract and the flagged run
    writes one (so its proof passed);
  - 12 grammars refused by all three with identical messages (6 at parser generation, 6 while reading `grammar.js`).
- **The schema.** All 77 contracts validate against the JSON schema (`check-jsonschema` 0.39.0), which rejects a field
  of the earlier format.
- **Tree-sitter's own suite.** `cargo xtask fetch-fixtures`, `cargo xtask generate-fixtures`, `cargo xtask test`.
  412 passed, 0 failed, 1 ignored, including the corpus tests of every fixture language (404 before the contract; the
  eight added are the generator's new tests).
- **Size and cost.** On the fixture grammars the contract is 3-17 times the size of `node-types.json` and at most a fifth
  of `parser.c`; with `--no-parser`, preparing it adds about 0.2 seconds to the largest grammar in a debug build.

Not done:

- Only rule trees are proven. Terminal sources, default aliases and subtypes are written from the generator's own
  structures (subtypes through the same function as `node-types.json`, terminal names cross-checked) without an
  equivalence proof.
- No fuzzing.
- The Wasm build and its tests (`cargo xtask generate-fixtures --wasm`, `cargo xtask test-wasm`) were not run; the change
  does not touch the runtime library or the Wasm bindings.

A published contract format is never changed in place: any change a consumer must know about is a new version of the
format string. Format 1 was published with the first four commits and is superseded by format 2; a consumer reading
format 1 pins a commit before the sixth.

## Continuous integration

`.github/workflows/contract.yml` runs on every `contract/**` branch: the generator's tests and lints, Tree-sitter's full
suite with fixtures regenerated by this CLI, and `fork/acceptance.sh` against the upstream CLI of the version the fork's
`tree-sitter-cli` declares. Upstream's own workflows are unchanged and run only on upstream's branches.

## Maintaining the fork across upstream releases

The branch layout: one branch per adopted upstream release, `contract/vX.Y.Z`, holding this series on that release's
tag. New work goes on the newest. Older ones are kept, and a published branch is never rewritten, so a consumer pinned to
a commit can always fetch it; consumers pin a commit, never a branch. No copy of upstream's `master` is published.

To adopt upstream release `vX.Y.Z`:

1. `git fetch origin --tags` and create `contract/vX.Y.Z` at tag `vX.Y.Z`.
2. Cherry-pick this series onto it, in order. Conflicts arise only in the files listed under Scope; resolve each so the
   refactored upstream functions still do exactly what upstream's do.
3. Read upstream's changes to `crates/generate/src/prepare_grammar/` and `node_types.rs` since the previous release, for
   the drift classes below.
4. Run every check under "How it has been verified" (the CI workflow runs them all), with the new upstream CLI as
   acceptance's reference.
5. Regenerate a consumer's contracts and compare them with the previous release's; explain every difference by an
   upstream change.
6. Move consumers' pins to the new branch's commit, and record the adoption here.

What can drift, and where it shows:

- **Preparation changes** (token extraction, repeat expansion, flattening, inlining, default aliases): the proof or the
  repetition check refuses a contract, or acceptance shows a difference. Either is fixed in this series, never by
  weakening a check.
- **Changed node-types computation** (supertypes, visibility): the subtype list follows automatically, since it is the
  same function; consumers see the change in the regenerated contracts.
- **New rule or symbol kinds** in `rules.rs`: the contract's exhaustive matches stop compiling until each is placed.
- **Changed public signatures** of `generate_parser_in_directory`: the flag's wiring needs re-applying.

The format string changes when the fork's output changes in a way a consumer must know: a field added, removed or
re-typed, or a changed meaning. An upstream change that only changes the values (a grammar now prepared differently)
does not change the format.

## Re-verifying by hand

1. Build the CLI: `cargo build --locked -p tree-sitter-cli`.
2. Run the generator's tests and lints (above).
3. Run Tree-sitter's own suite: `cargo xtask fetch-fixtures`, `cargo xtask generate-fixtures`, `cargo xtask test`.
4. Install the upstream CLI of the same version (`cargo install tree-sitter-cli --locked --version X.Y.Z --root DIR`)
   and run `fork/acceptance.sh DIR/bin/tree-sitter target/debug/tree-sitter WORK_DIR`.
