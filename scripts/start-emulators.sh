#!/bin/bash

# Script to start Firestore and Firebase Auth emulators for local development.
# Handles graceful shutdown and cleans up orphaned processes.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

FIRESTORE_HOST="127.0.0.1"
FIRESTORE_PORT="8092"
AUTH_PORT="9099"
UI_PORT="4000"

FIRESTORE_PID=""
FIREBASE_PID=""
EXIT_STATUS=0

cleanup() {
    echo ""
    echo "Shutting down emulators..."

    if [ -n "$FIREBASE_PID" ] && kill -0 "$FIREBASE_PID" 2>/dev/null; then
        echo "Stopping Firebase Auth emulator (PID $FIREBASE_PID)..."
        kill "$FIREBASE_PID" 2>/dev/null || true
        wait "$FIREBASE_PID" 2>/dev/null || true
    fi

    if [ -n "$FIRESTORE_PID" ] && kill -0 "$FIRESTORE_PID" 2>/dev/null; then
        echo "Stopping Firestore emulator (PID $FIRESTORE_PID)..."
        kill "$FIRESTORE_PID" 2>/dev/null || true
        wait "$FIRESTORE_PID" 2>/dev/null || true
    fi

    # The Firestore emulator spawns a Java subprocess that may outlive the parent.
    # Kill any remaining processes on our ports.
    kill_process_on_port "$FIRESTORE_PORT"
    kill_process_on_port "$AUTH_PORT"
    kill_process_on_port "$UI_PORT"

    echo "Emulators stopped."
    exit "$EXIT_STATUS"
}

trap cleanup SIGINT SIGTERM EXIT

get_pids_on_port() {
    local port="$1"
    if command -v lsof >/dev/null 2>&1; then
        lsof -ti :"$port" 2>/dev/null || true
    elif command -v ss >/dev/null 2>&1; then
        # Linux fallback using ss + /proc
        ss -tlnp 2>/dev/null | grep ":$port " | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | sort -u || true
    elif command -v netstat >/dev/null 2>&1; then
        # Older Linux fallback
        netstat -tlnp 2>/dev/null | grep ":$port " | awk '{print $7}' | cut -d'/' -f1 | grep -E '^[0-9]+$' || true
    fi
}

is_our_emulator_process() {
    local pid="$1"
    local port="$2"
    local cmd
    cmd=$(ps -p "$pid" -o command= 2>/dev/null || true)

    case "$port" in
        "$FIRESTORE_PORT")
            # Match the Java Firestore emulator process
            echo "$cmd" | grep -q "cloud-firestore-emulator" && return 0
            echo "$cmd" | grep -q "gcloud.*firestore" && return 0
            ;;
        "$AUTH_PORT"|"$UI_PORT")
            # Match Firebase emulator processes
            echo "$cmd" | grep -q "firebase" && return 0
            echo "$cmd" | grep -q "emulators" && return 0
            ;;
    esac
    return 1
}

kill_process_on_port() {
    local port="$1"
    local pids
    pids=$(get_pids_on_port "$port")

    if [ -n "$pids" ]; then
        for pid in $pids; do
            if is_our_emulator_process "$pid" "$port"; then
                echo "Killing orphaned emulator process on port $port (PID $pid)..."
                kill "$pid" 2>/dev/null || true
                # Give it a moment to die gracefully
                sleep 0.5
                # Force kill if still running
                if kill -0 "$pid" 2>/dev/null; then
                    kill -9 "$pid" 2>/dev/null || true
                fi
            fi
        done
    fi
}

wait_for_port() {
    local port="$1"
    local timeout_secs="${2:-30}"
    local attempt=0
    # Each iteration sleeps 0.5 seconds, so we need 2x timeout_secs iterations
    local max_attempts=$((timeout_secs * 2))

    while [ $attempt -lt $max_attempts ]; do
        if get_pids_on_port "$port" | grep -q .; then
            return 0
        fi
        sleep 0.5
        attempt=$((attempt + 1))
    done
    return 1
}

check_gcloud() {
    if ! command -v gcloud >/dev/null 2>&1; then
        echo "Error: gcloud CLI not found. Please install Google Cloud SDK."
        echo "Visit: https://cloud.google.com/sdk/docs/install"
        EXIT_STATUS=1
        exit 1
    fi
}

check_firebase() {
    if ! pnpm --filter @simlin/server exec which firebase >/dev/null 2>&1; then
        echo "Error: firebase CLI not found in @simlin/server."
        echo "Run 'pnpm install' to install dependencies."
        EXIT_STATUS=1
        exit 1
    fi
}

main() {
    cd "$PROJECT_DIR"

    echo "Checking prerequisites..."
    check_gcloud
    check_firebase

    # Clean up any orphaned emulator processes
    echo "Checking for orphaned emulator processes..."
    kill_process_on_port "$FIRESTORE_PORT"
    kill_process_on_port "$AUTH_PORT"
    kill_process_on_port "$UI_PORT"

    # Brief pause to ensure ports are released
    sleep 1

    echo "Starting Firestore emulator on $FIRESTORE_HOST:$FIRESTORE_PORT..."
    gcloud beta emulators firestore start --host-port="$FIRESTORE_HOST:$FIRESTORE_PORT" &
    FIRESTORE_PID=$!

    echo "Waiting for Firestore emulator to be ready..."
    if ! wait_for_port "$FIRESTORE_PORT" 30; then
        echo "Error: Firestore emulator failed to start within 30 seconds."
        EXIT_STATUS=1
        exit 1
    fi
    echo "Firestore emulator ready."

    echo "Starting Firebase Auth emulator on port $AUTH_PORT..."
    # Firebase CLI is installed in @simlin/server, but firebase.json is in src/app.
    # pnpm --filter overrides the working directory, so we pass --project and
    # --config explicitly instead of relying on cwd-based config discovery.
    pnpm --filter @simlin/server exec firebase emulators:start --only auth \
        --project simlin \
        --config "$PROJECT_DIR/src/app/firebase.json" &
    FIREBASE_PID=$!

    echo "Waiting for Firebase Auth emulator to be ready..."
    if ! wait_for_port "$AUTH_PORT" 30; then
        echo "Error: Firebase Auth emulator failed to start within 30 seconds."
        EXIT_STATUS=1
        exit 1
    fi
    echo "Firebase Auth emulator ready."

    echo ""
    echo "============================================"
    echo "Emulators running:"
    echo "  Firestore:     http://$FIRESTORE_HOST:$FIRESTORE_PORT"
    echo "  Firebase Auth: http://$FIRESTORE_HOST:$AUTH_PORT"
    echo "  Emulator UI:   http://$FIRESTORE_HOST:$UI_PORT"
    echo ""
    echo "Press Ctrl+C to stop all emulators."
    echo "============================================"
    echo ""

    # Wait for either process to exit. We use a polling loop instead of
    # `wait -n` because macOS ships with Bash 3.2 which lacks that feature.
    while true; do
        if ! kill -0 "$FIRESTORE_PID" 2>/dev/null; then
            echo "Firestore emulator exited unexpectedly."
            EXIT_STATUS=1
            break
        fi
        if ! kill -0 "$FIREBASE_PID" 2>/dev/null; then
            echo "Firebase Auth emulator exited unexpectedly."
            EXIT_STATUS=1
            break
        fi
        sleep 1
    done
}

main "$@"
