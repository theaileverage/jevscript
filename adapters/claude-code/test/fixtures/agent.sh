#!/bin/sh
# Stands in for `claude` in the integration test: shows what it was started
# with, then echoes every line it is sent until it is interrupted.
echo "agent started with: $*"
exec cat
