# The proxy sidecar of the restore Jobs, built from the CI runner's own binary,
# so the base has to carry the glibc the runner links against.
FROM ubuntu:24.04

# The proxy's HTTP client reads the system trust store and refuses to build
# without one, and this base carries no certificates. The bundle is the CI
# runner's own, copied in beside the binary, so the build never reaches an
# apt mirror: one mid-sync would fail every job of the matrix at once.
COPY ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY canopy-proxy /usr/bin/canopy-proxy
