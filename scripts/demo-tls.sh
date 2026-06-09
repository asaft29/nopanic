#!/usr/bin/env bash
#
# demo-tls.sh — Launch the full onion routing system with TLS enabled.
#
# Starts:  discovery (port 8080)
#          entry relay (port 9001) — TLS
#          middle relay (port 9002) — TLS
#          exit relay (port 9003) — TLS
#          tor-client SOCKS5 proxy (port 1080) — TLS
#
# Usage:
#   bash scripts/demo-tls.sh
#   curl --socks5 127.0.0.1:1080 http://example.com

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

DISCOVERY_PORT=8080
ENTRY_PORT=9001
MIDDLE_PORT=9002
EXIT_PORT=9003
SOCKS_PORT=1080
LOG_DIR="$ROOT_DIR/logs"
RUST_LOG="${RUST_LOG:-info}"

SKIP_BUILD=false
for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=true ;;
        --help|-h)
            echo "Usage: $0 [--skip-build]"
            echo "Launches the TLS onion routing demo on localhost."
            echo "Options:"
            echo "  --skip-build  Skip cargo build, use existing binaries"
            exit 0
            ;;
        *) echo "Unknown option: $arg (try --help)"; exit 1 ;;
    esac
done

PIDS=()

cleanup() {
    echo ""
    echo "==> Shutting down all services..."
    for pid in "${PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    sleep 1
    for pid in "${PIDS[@]}"; do
        kill -9 "$pid" 2>/dev/null || true
    done
    echo "==> All services stopped."
}
trap cleanup EXIT INT TERM

wait_for_tcp() {
    local host="$1" port="$2" name="$3" max="${4:-40}" attempt=0
    while [ "$attempt" -lt "$max" ]; do
        if bash -c "echo >/dev/tcp/$host/$port" 2>/dev/null; then return 0; fi
        attempt=$((attempt + 1))
        sleep 0.5
    done
    echo "ERROR: $name not listening on $host:$port after $max attempts"
    return 1
}

check_port() {
    local port="$1" name="$2"
    if ss -tlnp 2>/dev/null | grep -q ":${port} "; then
        echo "ERROR: Port $port already in use (needed for $name)"
        exit 1
    fi
}

section() {
    echo ""
    echo "──────────────────────────────────────────"
    echo "  $1"
    echo "──────────────────────────────────────────"
}

# ---- Pre-flight ----
check_port "$DISCOVERY_PORT" "discovery"
check_port "$ENTRY_PORT" "entry relay"
check_port "$MIDDLE_PORT" "middle relay"
check_port "$EXIT_PORT" "exit relay"
check_port "$SOCKS_PORT" "SOCKS5 proxy"

# ---- Build (TLS = default features) ----
if [ "$SKIP_BUILD" = false ]; then
    section "Building (TLS enabled, default features)"
    cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"
    echo "Build successful."
else
    echo "(Skipping build)"
fi

for bin in target/release/discovery target/release/relay-node target/release/tor-client; do
    if [ ! -x "$ROOT_DIR/$bin" ]; then
        echo "ERROR: Binary not found: $ROOT_DIR/$bin"
        exit 1
    fi
done

mkdir -p "$LOG_DIR"

# ---- Discovery ----
section "Starting Discovery (port $DISCOVERY_PORT)"
RUST_LOG="$RUST_LOG" "$ROOT_DIR/target/release/discovery" \
    > "$LOG_DIR/discovery.log" 2>&1 &
PIDS+=($!)
wait_for_tcp 127.0.0.1 "$DISCOVERY_PORT" "Discovery"

# ---- Relays ----
start_relay() {
    local node_type="$1" port="$2"
    section "Starting $node_type relay (port $port) — TLS"
    RUST_LOG="$RUST_LOG" "$ROOT_DIR/target/release/relay-node" \
        --node-type "$node_type" \
        --port "$port" \
        --host 127.0.0.1 \
        --directory-url "http://127.0.0.1:$DISCOVERY_PORT" \
        > "$LOG_DIR/relay-${node_type}.log" 2>&1 &
    PIDS+=($!)
    wait_for_tcp 127.0.0.1 "$port" "$node_type relay"
}

start_relay entry "$ENTRY_PORT"
start_relay middle "$MIDDLE_PORT"
start_relay exit "$EXIT_PORT"

echo ""
echo "Waiting for relays to register..."
sleep 5

# ---- Tor Client ----
section "Starting Tor Client (SOCKS5 on port $SOCKS_PORT) — TLS"

echo "  Launching tor-client with TUI dashboard..."
echo "  (Press 'q' or Ctrl+C to stop everything)"
echo ""
echo "  Mode:       TLS enabled"
echo "  Discovery:  127.0.0.1:$DISCOVERY_PORT"
echo "  Entry:      127.0.0.1:$ENTRY_PORT  (TLS)"
echo "  Middle:     127.0.0.1:$MIDDLE_PORT (TLS)"
echo "  Exit:       127.0.0.1:$EXIT_PORT   (TLS)"
echo "  SOCKS5:     127.0.0.1:$SOCKS_PORT"
echo "  Logs:       $LOG_DIR/"
echo ""
echo "  Test from another terminal:"
echo "    curl --socks5 127.0.0.1:$SOCKS_PORT http://example.com"
echo "    curl --socks5 127.0.0.1:$SOCKS_PORT http://httpbin.org/ip"
echo ""

RUST_LOG="$RUST_LOG" "$ROOT_DIR/target/release/tor-client" \
    --socks-addr "127.0.0.1:$SOCKS_PORT" \
    --directory-url "http://127.0.0.1:$DISCOVERY_PORT" \
    --pool-size 3 \
    --tui
