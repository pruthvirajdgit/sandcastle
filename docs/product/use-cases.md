# Sandcastle Use Cases

## 1. AI Coding Assistant — Code Execution

**Scenario**: An AI coding assistant (like Claude, GPT, or a custom agent) needs to run code to verify its solutions before presenting them to the user.

**Flow**:
```
User: "Write a function to sort a linked list"
Agent: [writes code] → execute_code(code, language="python") → sees output
Agent: [fixes bug] → execute_code(fixed_code) → tests pass
Agent: "Here's the solution, verified and working: ..."
```

**Config**:
- Isolation: `medium` (agent-generated code, mostly safe)
- Network: none
- Timeout: 30s
- Memory: 512MB

**Why Sandcastle**: Agent developers don't need to provision VMs or containers. One MCP config line and their agent can execute code safely.

---

## 2. Data Analysis Pipeline

**Scenario**: An agent receives a dataset from the user, writes analysis code, runs it, and returns results/visualizations.

**Flow**:
```
User: "Analyze this sales data" [attaches CSV, saved to /data/sales.csv]
Agent: create_sandbox(language="python", memory_mb=2048)
Agent: upload_file(sandbox_id, host_path="/data/sales.csv", sandbox_path="sales.csv")
Agent: execute_in_sandbox(sandbox_id, "import pandas as pd; df = pd.read_csv('sales.csv'); ...")
Agent: execute_in_sandbox(sandbox_id, "... generate charts, save to output.png ...")
Agent: download_file(sandbox_id, sandbox_path="output.png")
       → returns { host_path: "/output/sb-a1b2c3d4/output.png" }
Agent: destroy_sandbox(sandbox_id)
Agent: "Here's the analysis: [chart at /output/sb-a1b2c3d4/output.png] Revenue is up 23% QoQ..."
```

**Config**:
- Isolation: `medium`
- Network: `["pypi.org", "files.pythonhosted.org"]` (for pip install)
- Timeout: 300s (session)
- Memory: 2048MB

**Why Sandcastle**: Session persistence (state across multiple execute calls), file upload/download, and the agent never sees raw infrastructure.

---

## 3. Education — Student Code Evaluation

**Scenario**: An online learning platform uses an AI agent to grade student-submitted code assignments.

**Flow**:
```
Student submits: solution.py
Agent: execute_code(student_code, isolation="high", timeout=10)
Agent: Compares output with expected output
Agent: "Your solution produces correct output for 4/5 test cases. Test case 3 failed because..."
```

**Config**:
- Isolation: `high` (students may intentionally try to escape)
- Network: none (no cheating via internet)
- Timeout: 10s (prevent infinite loops)
- Memory: 256MB

**Why Sandcastle**: Students are adversarial — they might submit `import os; os.system("rm -rf /")` or try to read other students' submissions. Firecracker-level isolation ensures complete safety.

---

## 4. Automated Testing / CI Agent

**Scenario**: An AI agent generates tests for a codebase and runs them to verify correctness.

**Flow**:
```
Agent: create_sandbox(language="python", allowed_domains=["pypi.org"], timeout=300)
Agent: upload_file(sandbox_id, host_path="/repo/app.py", sandbox_path="app.py")
Agent: upload_file(sandbox_id, host_path="/repo/requirements.txt", sandbox_path="requirements.txt")
Agent: execute_in_sandbox(sandbox_id, "pip install -r requirements.txt")
Agent: execute_in_sandbox(sandbox_id, "python -m pytest test_app.py -v")
Agent: destroy_sandbox(sandbox_id)
```

**Config**:
- Isolation: `low` (own code, trusted)
- Network: pypi.org for dependencies
- Timeout: 300s
- Memory: 1024MB

**Why Sandcastle**: Agents can test code in isolation without polluting the host environment. Each test run starts clean.

---

## 5. Web Scraping / API Integration

**Scenario**: An agent needs to fetch data from specific APIs to answer user questions.

**Flow**:
```
User: "What's the weather in Tokyo?"
Agent: execute_code(
    code='import requests; r = requests.get("https://api.weather.gov/..."); print(r.json())',
    allowed_domains=["api.weather.gov"]
)
Agent: "The current weather in Tokyo is..."
```

**Config**:
- Isolation: `medium`
- Network: specific API domains only
- Timeout: 30s
- Memory: 256MB

**Why Sandcastle**: Network is blocked by default — even if the agent's code has a bug that tries to POST data somewhere else, Sandcastle blocks it. Only explicitly allowlisted domains are reachable.

---

## 6. Multi-Tenant Agent Platform

**Scenario**: A platform like OneClick.ai runs agents for multiple users. Each user's agent needs to execute code without accessing other users' data.

**Flow**:
```
Platform creates sandbox per user request:
  User A: create_sandbox(isolation="high") → sb-user-a
  User B: create_sandbox(isolation="high") → sb-user-b

User A's agent: execute_in_sandbox("sb-user-a", code_a)
User B's agent: execute_in_sandbox("sb-user-b", code_b)

# User A's code CANNOT access User B's sandbox — hardware-level isolation
```

**Config**:
- Isolation: `high` (multi-tenant = untrusted by definition)
- Network: per-user allowlist
- Timeout: per-user config
- Memory: per-user config

**Why Sandcastle**: Firecracker gives hardware-level isolation between tenants. Even a kernel exploit in User A's sandbox cannot reach User B's VM.

---

## 7. Code Review / Static Analysis Agent

**Scenario**: An agent reviews pull requests by actually running the code and tests, not just reading it.

**Flow**:
```
Agent: create_sandbox(language="javascript", allowed_domains=["registry.npmjs.org"])
Agent: upload_file(sandbox_id, host_path="/repo/package.json", sandbox_path="package.json")
Agent: upload_file(sandbox_id, host_path="/repo/src/index.js", sandbox_path="src/index.js")
Agent: upload_file(sandbox_id, host_path="/repo/test/index.test.js", sandbox_path="test/index.test.js")
Agent: execute_in_sandbox(sandbox_id, "npm install && npm test")
Agent: execute_in_sandbox(sandbox_id, "npx eslint src/")
Agent: destroy_sandbox(sandbox_id)
Agent: "Tests pass. ESLint found 2 warnings: ..."
```

---

## 8. Mathematical / Scientific Computation

**Scenario**: Agent needs to solve complex math problems with NumPy/SciPy.

**Flow**:
```
User: "Find the eigenvalues of this 100x100 matrix"
Agent: execute_code(
    code="import numpy as np; A = np.array([...]); print(np.linalg.eigvals(A))",
    language="python",
    isolation="low",
    memory_mb=1024
)
```

**Config**:
- Isolation: `low` (agent-generated, pure math)
- Network: none
- Timeout: 60s
- Memory: 1024MB

**Why Sandcastle**: Low isolation for speed (~5ms overhead), high memory for large matrices. No network needed.

---

## Integration Patterns

### Pattern 1: One-Shot (Most Common)
```
execute_code → get result → done
```
Use for: quick calculations, simple scripts, verifying code snippets.

### Pattern 2: Session-Based
```
create_sandbox → upload files → execute (multiple) → download results → destroy
```
Use for: data analysis, testing, multi-step workflows.

### Pattern 3: Long-Running
```
create_sandbox(timeout=3600) → execute periodically → destroy when done
```
Use for: monitoring scripts, background processing, development environments.
