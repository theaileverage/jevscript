#!/bin/sh
# Stands in for an agent CLI in the integration tests: shows what it was
# started with, then echoes every line it is sent until it is interrupted.
echo "agent started with: $*"
exec cat
