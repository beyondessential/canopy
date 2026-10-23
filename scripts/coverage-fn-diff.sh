#!/usr/bin/env bash
# List the functions one LCOV file covers that another no longer does.
#
# Why: after merging or deleting tests, the question is whether any production
# code lost its only test. A line diff answers it badly, because editing a file
# moves every line below the edit; function names do not move. Take a baseline
# with `just coverage` BEFORE editing tests (an instrumented build that is still
# running can pick up half-edited tests), copy target/coverage/lcov.info
# aside, edit, run `just coverage` again, then compare the two.
#
# Usage:
#   scripts/coverage-fn-diff.sh BASE.info NEW.info [PATH-REGEX]
#
# PATH-REGEX picks the source files to check (default: every crate's src/).
# Functions in inline test modules (a module named `tests`) and in files under
# tests/ are not reported: folding tests removes them on purpose.
#
# A function listed as no longer covered is a function no test now runs, or one
# instance of a generic function (each type it is called with is its own entry).
# For a generic, check that its body lines still run before worrying.
# Names are mangled; pipe through `rustfilt` if you have it.
set -euo pipefail

if [ "$#" -lt 2 ]; then
	echo "usage: $0 BASE.info NEW.info [PATH-REGEX]" >&2
	exit 64
fi

base=$1
new=$2
pattern=${3:-/crates/[^/]*/src/}

hits() {
	awk '
		/^SF:/ { file = substr($0, 4) }
		/^FNDA:/ {
			split(substr($0, 6), part, ",")
			total[file "\t" part[2]] += part[1]
		}
		END { for (key in total) print key "\t" total[key] }
	' "$1"
}

{
	hits "$base"
	echo "--NEW--"
	hits "$new"
} | awk -F'\t' -v pattern="$pattern" '
	/^--NEW--$/ { reading_new = 1; next }
	!reading_new { before[$1 FS $2] = $3; next }
	{ after[$1 FS $2] = $3 }
	END {
		for (key in before) {
			split(key, part, FS)
			if (part[1] !~ pattern || part[1] ~ /\/tests\// || part[2] ~ /5tests/) continue
			if (before[key] > 0) {
				checked++
				if (after[key] + 0 == 0) {
					lost++
					print part[1] ": " part[2]
				}
			}
		}
		printf "%d production functions covered before; %d no longer covered\n", checked, lost
	}
' | sort
