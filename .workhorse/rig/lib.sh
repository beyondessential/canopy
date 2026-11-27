# Sourced by every entry point.

# The rig runs steps with a home of its own, so the Rust tooling is pointed back
# at the person's: their cargo config (a compiler wrapper, say), registry cache
# and toolchains. Variables they set themselves come through env.fromPerson and
# win; these are the defaults for the ones they leave unset.
if [ -n "${RIG_PERSON_HOME:-}" ]; then
  if [ -z "${CARGO_HOME:-}" ] && [ -d "$RIG_PERSON_HOME/.cargo" ]; then
    export CARGO_HOME="$RIG_PERSON_HOME/.cargo"
  fi
  if [ -z "${RUSTUP_HOME:-}" ] && [ -d "$RIG_PERSON_HOME/.rustup" ]; then
    export RUSTUP_HOME="$RIG_PERSON_HOME/.rustup"
  fi

  # sccache keeps its cache under the cache home, which is the rig's own unless
  # the person set theirs. A server the rig starts is shared with the person's
  # own builds, so it caches where theirs would.
  case "${XDG_CACHE_HOME:-$HOME/.cache}" in
    "$HOME"/*) person_cache="$RIG_PERSON_HOME/.cache" ;;
    *) person_cache="$XDG_CACHE_HOME" ;;
  esac
  export SCCACHE_DIR="${SCCACHE_DIR:-$person_cache/sccache}"
  if [ -z "${SCCACHE_CONF:-}" ] && [ -f "$RIG_PERSON_HOME/.config/sccache/config" ]; then
    export SCCACHE_CONF="$RIG_PERSON_HOME/.config/sccache/config"
  fi
fi

# A server a compile starts lives in the step's cgroup and goes when the step
# ends, under the person's own builds that share it. So where sccache is the
# wrapper, the server is started in a scope of its own; one already running is
# left alone.
uses_sccache() {
  case "${RUSTC_WRAPPER-unset}" in
    *sccache*) return 0 ;;
    unset) ;;
    *) return 1 ;;
  esac
  case "${CARGO_BUILD_RUSTC_WRAPPER:-}" in *sccache*) return 0 ;; esac
  grep -qsE '^[[:space:]]*rustc-wrapper[[:space:]]*=.*sccache' \
    "${CARGO_HOME:-$HOME/.cargo}/config.toml" "${CARGO_HOME:-$HOME/.cargo}/config"
}
if uses_sccache && command -v sccache >/dev/null 2>&1 && command -v systemd-run >/dev/null 2>&1; then
  systemd-run --user --scope --collect --quiet sccache --start-server >/dev/null 2>&1 || true
fi

# With the person's rustup, rust-toolchain.toml picks the toolchain from those
# they have, and the rig leaves them as they are. Without it, rustup installs
# stable into the rig's home once, without docs, and later steps reuse it.
if [ -z "${RUSTUP_HOME:-}" ] && command -v rustup >/dev/null 2>&1; then
  rustup toolchain install stable --profile minimal --no-self-update >&2
  export RUSTUP_TOOLCHAIN=stable
fi

# build.rs would otherwise run npm ci and the SPA build inside cargo.
export SKIP_FRONTEND_BUILD=1
export CARGO_TERM_COLOR=never
# A rig build is built once, and sccache cannot cache an incremental compile.
export CARGO_INCREMENTAL=0

# The manager gives a step that touches the database both names. The server
# binary reads DATABASE_URL and psql is handed the other, so each is taken from
# the other rather than a step dying on the name it happens to want. Neither is
# set for deps or build, which touch no database.
if [ -n "${RIG_DATABASE_URL:-}" ] || [ -n "${DATABASE_URL:-}" ]; then
  RIG_DATABASE_URL="${RIG_DATABASE_URL:-$DATABASE_URL}"
  DATABASE_URL="${DATABASE_URL:-$RIG_DATABASE_URL}"
  export RIG_DATABASE_URL DATABASE_URL
fi

# The stand-ins for everything outside a Canopy instance, shared with the e2e
# fixture so the two cannot drift. rig.json promises a tester there is no AWS,
# Kubernetes, Tailscale control plane or Slack behind the lease.
export_fake_deps() {
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in '' | '#'*) continue ;; esac
    export "$line"
  done < "$RIG_SOURCE/scripts/fake-deps.env"
}

# The binaries a lease runs. deps warms the dependency build with exactly the
# set build compiles, so a binary named in one and not the other costs a
# recompile of what the shared build was meant to cover.
cargo_build_rig_bins() {
  set --
  for bin in private-server migrate seed; do
    set -- "$@" --bin "$bin"
  done
  cargo build --locked "$@"
}

# What the seed step leaves for the lease: the seeded device's certificate.
RIG_OUT="$RIG_SOURCE/.rig-out"

# The manager picks RIG_PORT without holding it, so the OS can hand it out again.
free_port() {
  node -e '
    const s = require("node:net").createServer().listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => process.exit(String(port) === process.env.RIG_PORT ? 1 : (console.log(port), 0)));
    });' || free_port
}

# Both servers have to be up inside the lease's ready window, so a wait is
# budgeted to a fraction of it. One that outlived the window would be killed
# mid-retry, with nothing said about which server never came up.
wait_for() {
  curl -fso /dev/null --max-time 2 --retry 20 --retry-all-errors --retry-delay 1 "$2" && return
  echo "serve: $1 did not answer at $2 within a minute" >&2
  exit 1
}
