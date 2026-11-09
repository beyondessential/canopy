# The proxy sidecar of the restore Jobs, built from the CI runner's own binary,
# so the base has to carry the glibc the runner links against.
FROM ubuntu:24.04
COPY canopy-proxy /usr/bin/canopy-proxy
