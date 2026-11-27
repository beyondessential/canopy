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

# What the build and seed leave for the lease: binaries, when target is shared
# with other builds (see build), and the seeded device's certificate.
RIG_OUT="$RIG_SOURCE/.rig-out"

bin() {
  if [ -x "$RIG_OUT/$1" ]; then
    printf '%s/%s' "$RIG_OUT" "$1"
  else
    printf '%s/target/debug/%s' "$RIG_SOURCE" "$1"
  fi
}

# The manager picks RIG_PORT without holding it, so the OS can hand it out again.
free_port() {
  node -e '
    const s = require("node:net").createServer().listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => process.exit(String(port) === process.env.RIG_PORT ? 1 : (console.log(port), 0)));
    });' || free_port
}

wait_for() { curl -fso /dev/null --max-time 5 --retry 120 --retry-all-errors --retry-delay 1 "$1"; }
