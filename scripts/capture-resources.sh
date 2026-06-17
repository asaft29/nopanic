#!/bin/bash
# Capture CPU and memory usage for all relay services.
# Run this alongside `make demo` in another terminal.
# Usage: bash scripts/capture-resources.sh [output_file]

OUTFILE="${1:-resource-usage.csv}"

echo "timestamp,pid,comm,%cpu,%mem" > "$OUTFILE"

while true; do
    TIMESTAMP=$(date +%s)
    ps -eo pid,comm:30,%cpu,%mem --no-headers 2>/dev/null \
        | grep -E "discovery|relay-node|tor-client" \
        | while read pid comm cpu mem; do
            echo "$TIMESTAMP,$pid,$comm,$cpu,$mem" >> "$OUTFILE"
          done
    sleep 2
done
