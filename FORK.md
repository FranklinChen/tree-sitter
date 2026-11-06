# This fork: the grammar contract, metadata and runtime repairs

This branch, `contract/v0.27.1`, is Tree-sitter at tag `v0.27.1` plus an opt-in feature: `tree-sitter generate
--grammar-contract` also writes `src/grammar-contract.json`, a machine-readable statement of the prepared grammar the
parser is generated from. The flag itself preserves the other generated artifacts. Separately, the fork corrects
metadata for start rules and carries the narrowly scoped native recovery repair documented below. Neither
unflagged generation nor runtime behavior is claimed to be identical to upstream for every grammar and input.
The contract feature is not part of upstream Tree-sitter, and there is no plan to propose it there. It is kept small
and in upstream's style anyway, because that is
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
- **Internal-token recovery progress.** The native runtime distinguishes an internal lexer's no-match, advancing,
  empty and EOF observations. Recovery skips unrecognized source instead of repeatedly accepting an empty internal
  token. Normal nullable tokens and external-scanner handling retain their behavior. Cached-token admission applies
  the same recovery policy.
- **Start-rule metadata.** `node-types.json` retains the start symbol's aliases and names its root by the default
  alias, when one exists. Root membership comes from the root's actual name and namedness; contextual aliases do not
  inherit it merely by sharing the start symbol. This changes schema metadata with or without the contract flag, not
  the generated parser or grammar admission. A start rule declared as a supertype also retains its root marker,
  matching the runtime's root wrapper.

The contract flag does not change grammar admission, generated `parser.c`, `node-types.json`, `grammar.json` or headers
relative to the same fork without the flag. Upstream comparison must account separately for the start-rule metadata
corrections. The runtime repair changes no public C, Rust or Wasm binding API, and does not reject nullable
grammars. Commands that parse with this fork's runtime inherit the recovery repair. Generated-parser byte equality is
a separate guarantee from runtime behavioral qualification.

Files: `crates/generate/src/contract.rs` (the writer and its proof), `crates/generate/src/prepare_grammar/contract_structure.rs`
(the rule trees and their paths), `flatten_grammar.rs`, `process_inlines.rs`, `node_types.rs`, `prepare_grammar.rs` and
`generate.rs` (the shared code and wiring), `crates/cli/src/main.rs` (the flag), the book pages above,
`docs/src/cli/generate.md`, this file, `fork/acceptance.sh` and `.github/workflows/contract.yml`. The runtime repair is
owned by `lib/src/parser.c`, with native regression controls in `crates/cli/src/tests/parser_test.rs`.

## Start-rule metadata corrections

For a start rule `source: $ => seq('a', optional($.nested))` whose only reference is
`nested: $ => seq('[', alias($.source, $.inner), ']')`, the runtime names the root `inner`. Upstream 0.27.1 instead
describes that root as `source` in `node-types.json`. The fork's shared node-type writer describes `inner`, and the
contract reads that corrected inventory. This was introduced with contract format 5, before the native recovery repair.

A November 6 comparison of this grammar confirms the intended difference: `node-types.json` differs from upstream;
`parser.c`, `grammar.json` and all three generated headers are identical. All six artifacts are identical between the
fork's unflagged and contract-flagged runs. A contextual-alias control keeps the root's original name and restores the
other aliases' metadata entries, which upstream omits. Those entries are not additional root kinds. A private
`RootNodeIdentity`, derived once from the start symbol and its default alias, admits root membership after entry
collection and merging. This removes the incorrect inference that every alias of rule zero is a root. Namedness is
part of the identity, so an anonymous alias with the root's spelling cannot inherit its root marker either.

The same identity rule retains the root marker when the start rule is declared as a supertype. For
`source: $ => $.item`, `item: $ => 'a'`, with `source` in `supertypes`, native parsing still produces `source` as
the root around `item`. Upstream's metadata omits that root marker; the fork preserves it on the supertype entry.

`fork/acceptance.sh` retains strict byte equality on its existing fixture population. Passing that population does not
establish universal equality with upstream or erase this documented metadata correction.

## Native recovery progress repair

The grammar `seq('a', /x*/, 'b')` admits a legitimate empty token in a normal parse. On `a!b`, upstream 0.27.1 can
repeatedly wrap an empty token in an ERROR without advancing past `!`. The repair classifies internal lexical
observations before token admission. EOF remains a terminating observation; an empty non-EOF token is admitted only
outside recovery. In recovery, speculative lexer reads are rewound before the existing decoded-character skip path
consumes unrecognized source. Its lookahead evidence is retained. Cached and incrementally reused internal tokens
cannot bypass this policy through an identical lexer mode. Recovery reuse examines the actual first leaf, so later
children cannot supply an empty internal leaf with their input width or external-scanner provenance.

The repair preserves syntactic optionality, nullable regexes in normal parsing, tokens whose nullable subexpressions
are followed by consuming content, and the existing external-scanner state/progress rules. It does not establish
termination for every grammar or scanner. The regression deck has a cooperative callback cutoff, so the known old
loop fails without an unbounded test run. It covers nullable, quoted, optional and consuming tokens, speculative
lookahead, UTF-8 error ranges and end-of-input contexts; it is not a hard process deadline.

The November 2 runtime checks and their limits are recorded below, separately from historical generator acceptance.
A consumer using the registry `tree-sitter` runtime does not receive this repair merely by pinning the fork's generator
CLI. It must deliberately select a runtime revision containing the repair.

## How the contract is proven

Each variable's rule tree is the tree just before flattening, with every inline reference replaced by the inline
variable's tree (the reference's alias and field winning, as inlining applies them), alternatives that match nothing
pruned, and reserved-word contexts and precedence dropped. The generator enumerates the tree's paths with its own
flattening code and compares them with the variable's final productions after inlining, step by step (symbol, alias with
default aliases restored, field) and in the end-of-input requirement. Any difference refuses the contract.

One shape is refused by design: an `eof()` followed, within its own rule, by an inline reference that may match nothing.
Flattening drops that path because the `eof()` is not its rule's last step, which the spliced tree cannot show. No
grammar among Tree-sitter's fixtures has this shape.

## Initial contract series

The initial series through contract format 4 comprised nine commits on top of `v0.27.1`, by subject:

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
8. `feat(generate)!: list the node kinds (contract format 3)`: the contract states every node kind, as `node-types.json`
   lists them and computed by the same code, so a consumer never re-derives which visible rules and tokens are nodes of
   their own name (one every reference renames is not). The one addition to `node_types.rs` is a `node_kinds` helper
   beside `supertype_subtypes`.
9. `feat(generate)!: state the grammar's name (contract format 4)`: the contract names its grammar, so a consumer that
   checks which grammar a file belongs to needs nothing beside the contract.

Subsequent format 5 repairs preserve root aliases, publish builtin symbol names, and include referenced-only node
kinds in the producer's inventory. The native recovery repair above is a separate runtime change; it does not change
contract format 5 or generated parser bytes.

## How it has been verified

### Native release qualification, November 6, 2026

The runtime repair at `ee2775065df9bfc3c401a7ae934e2e8552145142` passed the maintained native procedure on macOS ARM64:
fetch fixture grammars, regenerate their parsers with the fork CLI, then `cargo xtask test -- --locked`. The full native
suite passed **417 tests**, with **zero failures and one ignored doctest**: 319 CLI/native/corpus tests, 96 generator
tests and two tag tests. The 53 parser tests below are included in that total, not an additional run.

The acceptance checks compared **77 identical generations and 12 identical refusals** against upstream
0.27.1. Its fixture population does not contain the default-root-alias counterexample documented above. These results
qualify the native default-feature path, not Wasm execution, other platforms, fuzzing, publication or consumer adoption.
The optional Rust `wasm` feature and the browser bindings require their own build and execution checks before a
consumer of those features claims support for this repair.

The subsequent root-identity metadata correction passed all **97 generator tests**, generator and CLI all-target
Clippy, and the generator's build without default features. Native parsing of the default-alias, contextual-alias and
same-spelling anonymous-alias controls agrees with each generated root identity. For all three controls, parser and
header bytes remain equal to upstream; the contract flag preserves all six ordinary generated artifacts. The native
runtime source is unchanged by this metadata correction. A separate start-supertype control likewise confirms the
retained native root wrapper and its corrected metadata marker.

### Native recovery qualification, November 2, 2026

The native parser test module passes all **53 tests**, including two new tests with **31 authored scenarios**:

- 25 cold inputs across five grammars: 11 valid, nine recovery, and five incomplete/end-of-input cases.
- Six correctly edited incremental transitions, each compared with a cold parse for CST structure and every visible
  node's byte range. These exercise nested parents with a nullable first leaf, distant prefix changes, valid repairs,
  and consuming-first controls.

The first-leaf admission refinement closes a source-level mismatch found during review; the incremental controls are
not evidence of a separately reproduced hang. The cooperative cutoff bounds the known recovery loop in these tests,
not arbitrary native execution. This batch did not run the full upstream suite, Wasm execution, or fuzzing, and does
not establish release or consumer-deployment status.

### Initial contract qualification, October 30, 2026

On the historical contract series:

- **Generator tests and lints.** `cargo test -p tree-sitter-generate --lib` (94 tests, among them the proof's negative
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
  413 passed, 0 failed, 1 ignored, including the corpus tests of every fixture language (404 before the contract; the
  nine added are the generator's new tests).
- **Size and cost.** On the fixture grammars the contract is 3-17 times the size of `node-types.json` and at most a fifth
  of `parser.c`; with `--no-parser`, preparing it adds about 0.2 seconds to the largest grammar in a debug build.

Not done:

- Only rule trees are proven. Terminal sources, default aliases and subtypes are written from the generator's own
  structures (subtypes through the same function as `node-types.json`, terminal names cross-checked) without an
  equivalence proof.
- No fuzzing.
- The Wasm build and its tests (`cargo xtask generate-fixtures --wasm`, `cargo xtask test-wasm`) were not run. That
  historical contract-only series did not touch the runtime library or the Wasm bindings.

A published contract format is never changed in place: any change a consumer must know about is a new version of the
format string. Format 1 was published with the first four commits, format 2 with the sixth, format 3 with the eighth,
and format 4 with the ninth; a consumer pins the commit whose format it reads.

## Continuous integration

`.github/workflows/contract.yml` runs on every `contract/**` branch: the generator's tests and lints, Tree-sitter's full
native default-feature suite with fixtures regenerated by this CLI, and `fork/acceptance.sh` against the upstream CLI of the version the fork's
`tree-sitter-cli` declares. Upstream's own workflows are unchanged and run only on upstream's branches.

## Maintaining the fork across upstream releases

The branch layout: one branch per adopted upstream release, `contract/vX.Y.Z`, holding this series on that release's
tag. New work goes on the newest. Older ones are kept, and a published branch is never rewritten, so a consumer pinned to
a commit can always fetch it; consumers pin a commit, never a branch. No copy of upstream's `master` is published.

To adopt upstream release `vX.Y.Z`:

1. `git fetch origin --tags` and create `contract/vX.Y.Z` at tag `vX.Y.Z`.
2. Cherry-pick this series onto it, in order. Conflicts arise in the files listed under Scope; preserve the contract's
   generation equivalence separately from the runtime repair. If upstream has fixed the recovery defect, evaluate the
   regression deck before retaining or dropping the local runtime change.
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
