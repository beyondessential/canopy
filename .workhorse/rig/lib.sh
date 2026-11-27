# Sourced by every entry point.

# The rig runs steps with a home of its own, so rustup installs the toolchain
# there once, without docs, and later steps reuse it.
if command -v rustup >/dev/null 2>&1; then
  rustup toolchain install stable --profile minimal --no-self-update >&2
  export RUSTUP_TOOLCHAIN=stable
fi

# build.rs would otherwise run npm ci and the SPA build inside cargo.
export SKIP_FRONTEND_BUILD=1
export CARGO_TERM_COLOR=never
# A rig build is built once, and sccache cannot cache an incremental compile.
export CARGO_INCREMENTAL=0

# sccache is the machine's, shared with every build on it, only when the machine
# says where its cache is (SCCACHE_DIR, given to the rig through passEnv).
# Otherwise sccache, found through a parent directory's cargo config, would start
# a server of the rig's own on the shared port, caching into the rig's home, and
# the person's own builds would then use that server.
case "${RUSTC_WRAPPER:-}" in
  "" | *sccache*)
    if [ -n "${SCCACHE_DIR:-}" ] && command -v sccache >/dev/null 2>&1; then
      export RUSTC_WRAPPER=sccache
    else
      export RUSTC_WRAPPER=
    fi
    ;;
esac

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

wait_for() { curl -fso /dev/null --max-time 5 --retry 120 --retry-all-errors --retry-delay 1 "$1"; }
