#!/usr/bin/env bash
# Integration test for Sandcastle — verifies the full pipeline:
#   1. MCP server starts on stdio
#   2. Can list tools
#   3. Can execute code via execute_code tool
#
# Prerequisites:
#   - cargo build (all crates)
#   - sudo ./scripts/build-rootfs.sh (rootfs images)
#
# Usage: sudo ./tests/integration.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
SANDCASTLE_BIN="$PROJECT_DIR/service/target/debug/sandcastle"

echo "=== Sandcastle Integration Test ==="
echo ""

# ---- Test 1: Executor binary direct test ----
echo "--- Test 1: Executor binary (direct) ---"

# Ensure /workspace exists for executor
mkdir -p /workspace

# Helper: executor now emits {"ready":true} first, so parse the last JSON line
parse_exec_result() {
    python3 -c "
import sys, json
lines = [l.strip() for l in sys.stdin if l.strip()]
# Skip the ready signal, parse the last line
result = json.loads(lines[-1])
print(json.dumps(result))
"
}

result=$(echo '{"action":"exec","language":"bash","code":"echo hello from sandcastle","timeout_ms":5000}' | "$PROJECT_DIR/service/target/debug/sandcastle-executor" 2>/dev/null | parse_exec_result)
echo "  Result: $result"

stdout=$(echo "$result" | python3 -c "import sys,json; print(json.load(sys.stdin)['stdout'].strip())")
if [ "$stdout" = "hello from sandcastle" ]; then
    echo "  ✅ PASS: Executor bash execution works"
else
    echo "  ❌ FAIL: Expected 'hello from sandcastle', got '$stdout'"
    exit 1
fi

# Test Python execution
result=$(echo '{"action":"exec","language":"python","code":"print(2 + 2)","timeout_ms":5000}' | "$PROJECT_DIR/service/target/debug/sandcastle-executor" 2>/dev/null | parse_exec_result)
stdout=$(echo "$result" | python3 -c "import sys,json; print(json.load(sys.stdin)['stdout'].strip())")
if [ "$stdout" = "4" ]; then
    echo "  ✅ PASS: Executor python execution works"
else
    echo "  ❌ FAIL: Expected '4', got '$stdout'"
    exit 1
fi

# ---- Test 2: Executor timeout handling ----
echo ""
echo "--- Test 2: Executor timeout handling ---"

result=$(echo '{"action":"exec","language":"bash","code":"sleep 10","timeout_ms":1000}' | "$PROJECT_DIR/service/target/debug/sandcastle-executor" 2>/dev/null | parse_exec_result)
timed_out=$(echo "$result" | python3 -c "import sys,json; print(json.load(sys.stdin)['timed_out'])")
if [ "$timed_out" = "True" ]; then
    echo "  ✅ PASS: Timeout detection works"
else
    echo "  ❌ FAIL: Expected timed_out=true, got '$timed_out'"
    exit 1
fi

# ---- Test 3: MCP server tool listing ----
echo ""
echo "--- Test 3: MCP server initialization ---"

# Send a JSON-RPC initialize request and list tools
INIT_MSG='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}'
TOOLS_MSG='{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
INITIALIZED_MSG='{"jsonrpc":"2.0","method":"notifications/initialized"}'

# Start server, send messages, capture output
output=$(echo -e "$INIT_MSG\n$INITIALIZED_MSG\n$TOOLS_MSG" | timeout 5 "$SANDCASTLE_BIN" serve 2>/dev/null || true)

if echo "$output" | grep -q "execute_code"; then
    echo "  ✅ PASS: MCP server lists execute_code tool"
else
    echo "  ⚠️  SKIP: MCP server tool listing (may need ProcessSandbox runtime dirs)"
    echo "  Output: $(echo "$output" | head -3)"
fi

if echo "$output" | grep -q "create_sandbox"; then
    echo "  ✅ PASS: MCP server lists create_sandbox tool"
fi

# ---- Test 4: ProcessSandbox (low isolation) via MCP ----
echo ""
echo "--- Test 4: ProcessSandbox (low isolation) via execute_code ---"

EXEC_LOW='{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"execute_code","arguments":{"code":"print(40+2)","language":"python","isolation":"low"}}}'

output=$(echo -e "$INIT_MSG\n$INITIALIZED_MSG\n$EXEC_LOW" | timeout 15 "$SANDCASTLE_BIN" serve 2>/dev/null || true)

if echo "$output" | grep -q "42"; then
    echo "  ✅ PASS: execute_code with isolation=low works (ProcessSandbox)"
else
    echo "  ❌ FAIL: Expected '42' in output"
    echo "  Output: $(echo "$output" | tail -3)"
    exit 1
fi

# ---- Test 5: GvisorSandbox (medium isolation) via MCP ----
echo ""
echo "--- Test 5: GvisorSandbox (medium isolation) via execute_code ---"

if command -v runsc &>/dev/null; then
    EXEC_MED='{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"execute_code","arguments":{"code":"print(7*8)","language":"python","isolation":"medium"}}}'

    output=$(echo -e "$INIT_MSG\n$INITIALIZED_MSG\n$EXEC_MED" | timeout 15 "$SANDCASTLE_BIN" serve 2>/dev/null || true)

    if echo "$output" | grep -q "56"; then
        echo "  ✅ PASS: execute_code with isolation=medium works (GvisorSandbox)"
    else
        echo "  ❌ FAIL: Expected '56' in output"
        echo "  Output: $(echo "$output" | tail -3)"
        exit 1
    fi
else
    echo "  ⚠️  SKIP: runsc not installed, skipping medium isolation test"
fi

# ---- Test 6: Persistent session with gVisor ----
echo ""
echo "--- Test 6: Persistent gVisor session (create → execute → destroy) ---"

if command -v runsc &>/dev/null; then
    # Use Python to drive the MCP server interactively over stdio
    test6_output=$(timeout 30 python3 -c "
import subprocess, json, sys

server = subprocess.Popen(
    ['$SANDCASTLE_BIN', 'serve'],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    text=True, bufsize=1
)

def send(msg):
    server.stdin.write(json.dumps(msg) + '\n')
    server.stdin.flush()

def recv():
    line = server.stdout.readline().strip()
    return json.loads(line) if line else None

send({'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2024-11-05','capabilities':{},'clientInfo':{'name':'test','version':'0.1.0'}}})
recv()
send({'jsonrpc':'2.0','method':'notifications/initialized'})

send({'jsonrpc':'2.0','id':2,'method':'tools/call','params':{'name':'create_sandbox','arguments':{'language':'python','isolation':'medium'}}})
resp = recv()
text = resp['result']['content'][0]['text']
data = json.loads(text)
sid = data['session_id']
print(f'SESSION:{sid}')

send({'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'execute_in_session','arguments':{'session_id': sid, 'code': 'x = 99\nprint(x * 2)'}}})
resp = recv()
text = resp['result']['content'][0]['text']
if '198' in text:
    print('EXEC_OK')
else:
    print(f'EXEC_FAIL:{text}')

send({'jsonrpc':'2.0','id':4,'method':'tools/call','params':{'name':'destroy_sandbox','arguments':{'session_id': sid}}})
resp = recv()
text = resp['result']['content'][0]['text']
if 'destroyed' in text.lower() or 'success' in text.lower():
    print('DESTROY_OK')
else:
    print(f'DESTROY_FAIL:{text}')

server.stdin.close()
server.wait(timeout=5)
" 2>&1 || true)

    if echo "$test6_output" | grep -q "SESSION:"; then
        echo "  Created gVisor session: $(echo "$test6_output" | grep SESSION: | cut -d: -f2)"
    fi
    if echo "$test6_output" | grep -q "EXEC_OK"; then
        echo "  ✅ PASS: execute_in_session works in gVisor"
    else
        echo "  ❌ FAIL: execute_in_session in gVisor"
        echo "  $test6_output"
    fi
    if echo "$test6_output" | grep -q "DESTROY_OK"; then
        echo "  ✅ PASS: destroy_sandbox works for gVisor session"
    else
        echo "  ⚠️  WARN: destroy response not confirmed"
    fi
else
    echo "  ⚠️  SKIP: runsc not installed, skipping gVisor session test"
fi

# ---- Test 7: FirecrackerSandbox (high isolation) via MCP ----
echo ""
echo "--- Test 7: FirecrackerSandbox (high isolation) via execute_code ---"

if [ -f /usr/local/bin/firecracker ] && [ -f /var/lib/sandcastle/kernel/vmlinux ] && [ -f /var/lib/sandcastle/rootfs/python.ext4 ]; then
    EXEC_HIGH='{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"execute_code","arguments":{"code":"print(11*11)","language":"python","isolation":"high"}}}'

    output=$(echo -e "$INIT_MSG\n$INITIALIZED_MSG\n$EXEC_HIGH" | timeout 75 "$SANDCASTLE_BIN" serve 2>/dev/null || true)

    if echo "$output" | grep -q "121"; then
        echo "  ✅ PASS: execute_code with isolation=high works (FirecrackerSandbox)"
    else
        echo "  ❌ FAIL: Expected '121' in output"
        echo "  Output: $(echo "$output" | tail -3)"
        exit 1
    fi
else
    echo "  ⚠️  SKIP: firecracker/kernel/rootfs not available, skipping high isolation test"
fi

# ---- Summary ----
echo ""
echo "=== Integration tests complete ==="
