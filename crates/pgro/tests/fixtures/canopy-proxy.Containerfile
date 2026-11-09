# The proxy sidecar of the restore Jobs, built from the CI runner's own binary,
# so the base has to carry the glibc the runner links against.
FROM ubuntu:24.04

# The proxy's HTTP client reads the system trust store and refuses to build
# without one, and this base carries no certificates. The deployed image gets
# its bundle the same way, from a Debian-family base (see .github/Dockerfile.native).
RUN apt-get update \
	&& apt-get install -y --no-install-recommends ca-certificates \
	&& rm -rf /var/lib/apt/lists/*

COPY canopy-proxy /usr/bin/canopy-proxy
