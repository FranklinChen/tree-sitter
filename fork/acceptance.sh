#!/usr/bin/env bash
# The fork's acceptance check: it only ADDS src/grammar-contract.json, and only
# when asked to.
#
# For every Tree-sitter fixture grammar, upstream's CLI runs
# `tree-sitter generate src/grammar.json`, and this fork's CLI runs the same
# command twice, once as is and once with `--grammar-contract`, on the same
# input. In all three, parser.c, node-types.json, grammar.json and the
# tree_sitter/ headers must be byte-identical; the contract must be absent
# from the plain run and present in the other. A grammar upstream refuses must
# be refused by both fork runs with the same message.
#
# The inputs are the fetched upstream fixture grammars (run
# `cargo xtask fetch-fixtures` first), whose src/grammar.json is used as
# shipped, and the repository's test grammars, whose grammar.js upstream's
# CLI converts to grammar.json (a test grammar upstream refuses must be
# refused identically by the fork).
#
# Usage: fork/acceptance.sh UPSTREAM_CLI FORK_CLI WORK_DIR
# Exits non-zero on any difference, parity failure or missing contract.
set -euo pipefail

if [ "$#" -ne 3 ]; then
    printf 'usage: %s UPSTREAM_CLI FORK_CLI WORK_DIR\n' "$0" >&2
    exit 2
fi
upstream=$1
fork=$2
work=$3
repo=$(cd "$(dirname "$0")/.." && pwd)
fixtures="$repo/test/fixtures"

if [ ! -d "$fixtures/grammars/json/src" ]; then
    printf 'fetched fixtures are missing; run: cargo xtask fetch-fixtures\n' >&2
    exit 2
fi

rm -rf "$work"
mkdir -p "$work/inputs" "$work/cases"
status=0
compared=("parser.c" "node-types.json" "grammar.json" "tree_sitter/parser.h" "tree_sitter/alloc.h" "tree_sitter/array.h")
identical=0
refused_both=0

# normalized_log LOG DIR: LOG with its own directory written as one token and
# node's per-process prefix (`(node:<pid>)`, on its warnings) without the
# process id, the two parts of a refusal that differ from run to run.
normalized_log() {
    sed -e "s#$2#DIR#g" -e 's/(node:[0-9][0-9]*)/(node:PID)/g' "$1"
}

# same_refusal LOG_A DIR_A LOG_B DIR_B: the logs match once normalized. When
# they do not, the difference is printed (at most 40 lines), so a run that
# fails says why.
same_refusal() {
    if cmp -s <(normalized_log "$1" "$2") <(normalized_log "$3" "$4"); then
        return 0
    fi
    diff <(normalized_log "$1" "$2") <(normalized_log "$3" "$4") | head -n 40 || true
    return 1
}

# compare NAME GRAMMAR_JSON: generate from GRAMMAR_JSON with upstream's CLI
# ("upstream"), the fork's as is ("fork") and the fork's asked for the contract
# ("contract").
compare() {
    local name=$1 grammar=$2
    local case_dir="$work/cases/$name"
    local side
    for side in upstream fork contract; do
        mkdir -p "$case_dir/$side/src"
        cp "$grammar" "$case_dir/$side/src/grammar.json"
    done
    local upstream_ok=true fork_ok=true contract_ok=true
    (cd "$case_dir/upstream" && "$upstream" generate src/grammar.json > gen.log 2>&1) || upstream_ok=false
    (cd "$case_dir/fork" && "$fork" generate src/grammar.json > gen.log 2>&1) || fork_ok=false
    (cd "$case_dir/contract" && "$fork" generate --grammar-contract src/grammar.json > gen.log 2>&1) ||
        contract_ok=false
    local before=$status
    for side in fork contract; do
        local side_ok
        if [ "$side" = fork ]; then side_ok=$fork_ok; else side_ok=$contract_ok; fi
        case "$upstream_ok/$side_ok" in
            true/true) ;;
            false/false)
                if ! same_refusal "$case_dir/upstream/gen.log" "$case_dir/upstream" \
                    "$case_dir/$side/gen.log" "$case_dir/$side"; then
                    printf 'REFUSED-DIFFERENTLY %s by %s (see %s)\n' "$name" "$side" "$case_dir"
                    status=1
                fi
                ;;
            false/true)
                printf 'FORK-ACCEPTS %s by %s, which upstream refuses (see %s)\n' "$name" "$side" "$case_dir"
                status=1
                ;;
            true/false)
                printf 'FORK-REFUSES %s by %s (see %s)\n' "$name" "$side" "$case_dir/$side/gen.log"
                status=1
                ;;
        esac
    done
    if [ "$status" -ne "$before" ]; then
        return
    fi
    if [ "$upstream_ok" = false ]; then
        refused_both=$((refused_both + 1))
        return
    fi
    local differing=()
    local file
    for side in fork contract; do
        for file in "${compared[@]}"; do
            if ! cmp -s "$case_dir/upstream/src/$file" "$case_dir/$side/src/$file"; then
                differing+=("$side:$file")
            fi
        done
    done
    if [ "${#differing[@]}" -ne 0 ]; then
        printf 'DIFFERS %s: %s\n' "$name" "${differing[*]}"
        status=1
    elif [ -e "$case_dir/fork/src/grammar-contract.json" ]; then
        printf 'UNASKED contract: %s\n' "$name"
        status=1
    elif [ ! -s "$case_dir/contract/src/grammar-contract.json" ]; then
        printf 'MISSING contract: %s\n' "$name"
        status=1
    else
        identical=$((identical + 1))
    fi
}

# The fetched grammars' shipped grammar.json files (gitignored clones, so
# the search must not honor ignore files).
while IFS= read -r grammar; do
    name=${grammar#"$fixtures/grammars/"}
    name=${name%/src/grammar.json}
    compare "grammars__${name//\//__}" "$grammar"
done < <(find "$fixtures/grammars" -path '*/node_modules' -prune -o \
    -path '*/src/grammar.json' -type f -print | sort)

# The test grammars, converted by upstream; an upstream refusal must be the
# fork's refusal too.
while IFS= read -r grammar_js; do
    dir=$(dirname "$grammar_js")
    name="test_grammars__$(basename "$dir")"
    for side in upstream fork; do
        mkdir -p "$work/inputs/$name.$side"
        cp -R "$dir/." "$work/inputs/$name.$side/"
    done
    if (cd "$work/inputs/$name.upstream" && "$upstream" generate --no-parser > convert.log 2>&1); then
        compare "$name" "$work/inputs/$name.upstream/src/grammar.json"
    elif (cd "$work/inputs/$name.fork" && "$fork" generate --no-parser > convert.log 2>&1); then
        printf 'FORK-ACCEPTS %s, which upstream refuses to read\n' "$name"
        status=1
    elif same_refusal "$work/inputs/$name.upstream/convert.log" "$work/inputs/$name.upstream" \
        "$work/inputs/$name.fork/convert.log" "$work/inputs/$name.fork"; then
        refused_both=$((refused_both + 1))
    else
        printf 'REFUSED-DIFFERENTLY %s while reading grammar.js\n' "$name"
        status=1
    fi
done < <(find "$fixtures/test_grammars" -name grammar.js -type f | sort)

printf 'identical: %s; refused identically by both: %s\n' "$identical" "$refused_both"
exit "$status"
