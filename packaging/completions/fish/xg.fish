# fish completion for xg (xiaoguai CLI)
# Installation:
#   cp xg.fish ~/.config/fish/completions/xg.fish
#   cp xg.fish ~/.config/fish/completions/xiaoguai.fish   # if also using the long name
#
# With Homebrew: automatically installed to $(brew --prefix)/share/fish/vendor_completions.d/

# Disable file completion by default (we handle it explicitly where needed)
complete -c xg -f
complete -c xiaoguai -f

# ── helpers ──────────────────────────────────────────────────────────────────

# Returns true when no subcommand has been typed yet
function __xg_no_subcommand
    not __fish_seen_subcommand_from \
        chat provider mcp remote eval backup restore self-update audit \
        hotl outcomes skills watch anomaly
end

# Returns true when $argv[1] is the active top-level command
function __xg_using_cmd
    __fish_seen_subcommand_from $argv[1]
end

# Returns true when $argv[1] is the top-level cmd and $argv[2] is the sub
function __xg_using_subcmd
    __fish_seen_subcommand_from $argv[1] && __fish_seen_subcommand_from $argv[2]
end

# Returns true when $argv[1] is the top-level cmd, $argv[2] the sub, $argv[3] the action
function __xg_using_action
    __fish_seen_subcommand_from $argv[1] && __fish_seen_subcommand_from $argv[2] && __fish_seen_subcommand_from $argv[3]
end

# ── global flag ──────────────────────────────────────────────────────────────

complete -c xg -n '__xg_no_subcommand' -l config -d 'Path to YAML config file' -r
complete -c xg -n '__xg_no_subcommand' -l help   -d 'Show help'
complete -c xg -n '__xg_no_subcommand' -l version -d 'Show version'

# ── top-level subcommands ────────────────────────────────────────────────────

complete -c xg -n '__xg_no_subcommand' -a chat        -d 'Send a one-shot prompt to the agent'
complete -c xg -n '__xg_no_subcommand' -a provider    -d 'Manage LLM provider registry'
complete -c xg -n '__xg_no_subcommand' -a mcp         -d 'Manage MCP server registry'
complete -c xg -n '__xg_no_subcommand' -a remote      -d 'Talk to a running xiaoguai-api over HTTP/SSE'
complete -c xg -n '__xg_no_subcommand' -a eval        -d 'Run eval suite against mock backend'
complete -c xg -n '__xg_no_subcommand' -a backup      -d 'Create a backup archive'
complete -c xg -n '__xg_no_subcommand' -a restore     -d 'Restore a backup archive'
complete -c xg -n '__xg_no_subcommand' -a self-update -d 'Check for and apply binary updates'
complete -c xg -n '__xg_no_subcommand' -a audit       -d 'Audit log management'
# Wave-3
complete -c xg -n '__xg_no_subcommand' -a hotl        -d 'Hotl policy management (wave-3)'
complete -c xg -n '__xg_no_subcommand' -a outcomes    -d 'Outcome telemetry (wave-3)'
complete -c xg -n '__xg_no_subcommand' -a skills      -d 'Skill pack management (wave-3)'
complete -c xg -n '__xg_no_subcommand' -a watch       -d 'Watch rule management (wave-3)'
complete -c xg -n '__xg_no_subcommand' -a anomaly     -d 'Anomaly detection (wave-3)'

# ── chat ─────────────────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd chat' -l prompt     -d 'User prompt' -r
complete -c xg -n '__xg_using_cmd chat' -l mock       -d 'Use deterministic mock backend'
complete -c xg -n '__xg_using_cmd chat' -l ollama-url -d 'Override Ollama base URL' -r
complete -c xg -n '__xg_using_cmd chat' -l model      -d 'LLM model name' -r

# ── provider ─────────────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd provider; and not __fish_seen_subcommand_from register list remove' \
    -a register -d 'Register a new provider'
complete -c xg -n '__xg_using_cmd provider; and not __fish_seen_subcommand_from register list remove' \
    -a list     -d 'List providers'
complete -c xg -n '__xg_using_cmd provider; and not __fish_seen_subcommand_from register list remove' \
    -a remove   -d 'Remove a provider by id'

complete -c xg -n '__xg_using_subcmd provider register' -l name           -d 'Provider name' -r
complete -c xg -n '__xg_using_subcmd provider register' -l kind           -d 'Provider kind' -r -a 'ollama openai_compat'
complete -c xg -n '__xg_using_subcmd provider register' -l endpoint       -d 'Endpoint URL' -r
complete -c xg -n '__xg_using_subcmd provider register' -l models         -d 'Supported model names (comma-separated)' -r
complete -c xg -n '__xg_using_subcmd provider register' -l default-for    -d 'Default-for models (comma-separated)' -r
complete -c xg -n '__xg_using_subcmd provider register' -l fallback-order -d 'Fallback priority' -r
complete -c xg -n '__xg_using_subcmd provider register' -l api-key-env    -d 'Env var holding API key' -r
complete -c xg -n '__xg_using_subcmd provider register' -l tenant         -d 'Tenant id' -r

complete -c xg -n '__xg_using_subcmd provider list'   -l tenant -d 'Tenant id' -r
complete -c xg -n '__xg_using_subcmd provider remove' -l id     -d 'Provider id' -r

# ── mcp ──────────────────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd mcp; and not __fish_seen_subcommand_from register list remove' \
    -a register -d 'Register a new MCP server'
complete -c xg -n '__xg_using_cmd mcp; and not __fish_seen_subcommand_from register list remove' \
    -a list     -d 'List MCP servers'
complete -c xg -n '__xg_using_cmd mcp; and not __fish_seen_subcommand_from register list remove' \
    -a remove   -d 'Remove an MCP server by id'

complete -c xg -n '__xg_using_subcmd mcp register' -l name      -d 'Server name' -r
complete -c xg -n '__xg_using_subcmd mcp register' -l version   -d 'Version' -r
complete -c xg -n '__xg_using_subcmd mcp register' -l transport -d 'Transport type' -r -a 'stdio sse http'
complete -c xg -n '__xg_using_subcmd mcp register' -l command   -d 'Command to spawn' -r
complete -c xg -n '__xg_using_subcmd mcp register' -l args      -d 'Comma-separated args' -r
complete -c xg -n '__xg_using_subcmd mcp register' -l env-keys  -d 'Comma-separated env var names' -r
complete -c xg -n '__xg_using_subcmd mcp register' -l endpoint  -d 'Endpoint URL' -r
complete -c xg -n '__xg_using_subcmd mcp register' -l tenant    -d 'Tenant id' -r

complete -c xg -n '__xg_using_subcmd mcp list'   -l tenant -d 'Tenant id' -r
complete -c xg -n '__xg_using_subcmd mcp remove' -l id     -d 'Server id' -r

# ── remote ───────────────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd remote' -l server -d 'Base URL of the API server' -r

complete -c xg -n '__xg_using_cmd remote; and not __fish_seen_subcommand_from healthz chat messages cancel' \
    -a healthz  -d 'Smoke test the remote server'
complete -c xg -n '__xg_using_cmd remote; and not __fish_seen_subcommand_from healthz chat messages cancel' \
    -a chat     -d 'Send a prompt to a fresh session'
complete -c xg -n '__xg_using_cmd remote; and not __fish_seen_subcommand_from healthz chat messages cancel' \
    -a messages -d 'Fetch message history'
complete -c xg -n '__xg_using_cmd remote; and not __fish_seen_subcommand_from healthz chat messages cancel' \
    -a cancel   -d 'Cancel an in-flight agent run'

complete -c xg -n '__xg_using_subcmd remote chat' -l user-id   -d 'User id' -r
complete -c xg -n '__xg_using_subcmd remote chat' -l tenant-id -d 'Tenant id' -r
complete -c xg -n '__xg_using_subcmd remote chat' -l model     -d 'Model name' -r
complete -c xg -n '__xg_using_subcmd remote chat' -l prompt    -d 'Prompt text' -r
complete -c xg -n '__xg_using_subcmd remote chat' -l title     -d 'Session title' -r

complete -c xg -n '__xg_using_subcmd remote messages' -l session -d 'Session id' -r
complete -c xg -n '__xg_using_subcmd remote cancel'   -l session -d 'Session id' -r

# ── eval ─────────────────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd eval; and not __fish_seen_subcommand_from run' -a run -d 'Walk eval cases and grade them'

complete -c xg -n '__xg_using_subcmd eval run' -l suite          -d 'Suite name' -r
complete -c xg -n '__xg_using_subcmd eval run' -l cases-dir      -d 'Directory of .eval.yaml files' -r -F
complete -c xg -n '__xg_using_subcmd eval run' -l out            -d 'Output JSON report path' -r -F
complete -c xg -n '__xg_using_subcmd eval run' -l max-iterations -d 'Max agent iterations' -r

# ── backup ───────────────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd backup' -l out          -d 'Output .tar.gz path' -r -F
complete -c xg -n '__xg_using_cmd backup' -l database-url -d 'PostgreSQL connection URL' -r
complete -c xg -n '__xg_using_cmd backup' -l encrypt      -d 'Age public-key file for encryption' -r -F

# ── restore ──────────────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd restore' -l in       -d 'Path to backup archive' -r -F
complete -c xg -n '__xg_using_cmd restore' -l outdir   -d 'Extraction directory' -r -F
complete -c xg -n '__xg_using_cmd restore' -l force    -d 'Overwrite existing output directory'
complete -c xg -n '__xg_using_cmd restore' -l identity -d 'Age identity file for decryption' -r -F

# ── self-update ───────────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd self-update' -l check -d 'Only report if update available'

# ── audit ─────────────────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd audit; and not __fish_seen_subcommand_from export' -a export -d 'Export audit rows to S3-compatible sink'

complete -c xg -n '__xg_using_subcmd audit export' -l sink         -d 'Sink type' -r -a 's3'
complete -c xg -n '__xg_using_subcmd audit export' -l bucket       -d 'S3 bucket name' -r
complete -c xg -n '__xg_using_subcmd audit export' -l prefix       -d 'S3 key prefix' -r
complete -c xg -n '__xg_using_subcmd audit export' -l region       -d 'AWS region' -r
complete -c xg -n '__xg_using_subcmd audit export' -l endpoint-url -d 'Endpoint URL for MinIO/localstack' -r
complete -c xg -n '__xg_using_subcmd audit export' -l sink-name    -d 'Logical sink name' -r
complete -c xg -n '__xg_using_subcmd audit export' -l interval-secs -d 'Export interval in seconds' -r
complete -c xg -n '__xg_using_subcmd audit export' -l once         -d 'Run one cycle then exit'
complete -c xg -n '__xg_using_subcmd audit export' -l database-url -d 'Postgres connection URL' -r

# ── hotl (wave-3) ─────────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd hotl; and not __fish_seen_subcommand_from policy check' \
    -a policy -d 'Manage hotl policies'
complete -c xg -n '__xg_using_cmd hotl; and not __fish_seen_subcommand_from policy check' \
    -a check  -d 'Check hotl policy status for a session'

# hotl policy actions
complete -c xg -n '__xg_using_subcmd hotl policy; and not __fish_seen_subcommand_from create list get update delete' \
    -a create -d 'Create a new hotl policy'
complete -c xg -n '__xg_using_subcmd hotl policy; and not __fish_seen_subcommand_from create list get update delete' \
    -a list   -d 'List hotl policies'
complete -c xg -n '__xg_using_subcmd hotl policy; and not __fish_seen_subcommand_from create list get update delete' \
    -a get    -d 'Get a hotl policy by id'
complete -c xg -n '__xg_using_subcmd hotl policy; and not __fish_seen_subcommand_from create list get update delete' \
    -a update -d 'Update a hotl policy'
complete -c xg -n '__xg_using_subcmd hotl policy; and not __fish_seen_subcommand_from create list get update delete' \
    -a delete -d 'Delete a hotl policy'

complete -c xg -n '__xg_using_action hotl policy create' -l name     -d 'Policy name' -r
complete -c xg -n '__xg_using_action hotl policy create' -l rule     -d 'Policy rule expression' -r
complete -c xg -n '__xg_using_action hotl policy create' -l priority -d 'Priority (lower = higher precedence)' -r
complete -c xg -n '__xg_using_action hotl policy create' -l enabled  -d 'Enable policy'

complete -c xg -n '__xg_using_action hotl policy list' -l output -d 'Output format' -r -a 'table json yaml'
complete -c xg -n '__xg_using_action hotl policy list' -l limit  -d 'Max results' -r
complete -c xg -n '__xg_using_action hotl policy list' -l offset -d 'Pagination offset' -r

complete -c xg -n '__xg_using_action hotl policy get' -l id     -d 'Policy id' -r
complete -c xg -n '__xg_using_action hotl policy get' -l output -d 'Output format' -r -a 'table json yaml'

complete -c xg -n '__xg_using_action hotl policy update' -l id       -d 'Policy id' -r
complete -c xg -n '__xg_using_action hotl policy update' -l name     -d 'New name' -r
complete -c xg -n '__xg_using_action hotl policy update' -l rule     -d 'New rule expression' -r
complete -c xg -n '__xg_using_action hotl policy update' -l priority -d 'New priority' -r
complete -c xg -n '__xg_using_action hotl policy update' -l enabled  -d 'Enable policy'

complete -c xg -n '__xg_using_action hotl policy delete' -l id    -d 'Policy id' -r
complete -c xg -n '__xg_using_action hotl policy delete' -l force -d 'Skip confirmation'

complete -c xg -n '__xg_using_subcmd hotl check' -l tenant  -d 'Tenant id' -r
complete -c xg -n '__xg_using_subcmd hotl check' -l session -d 'Session id' -r
complete -c xg -n '__xg_using_subcmd hotl check' -l output  -d 'Output format' -r -a 'table json yaml'

# ── outcomes (wave-3) ─────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd outcomes; and not __fish_seen_subcommand_from record list summary timeseries' \
    -a record     -d 'Record an outcome for a session'
complete -c xg -n '__xg_using_cmd outcomes; and not __fish_seen_subcommand_from record list summary timeseries' \
    -a list       -d 'List recorded outcomes'
complete -c xg -n '__xg_using_cmd outcomes; and not __fish_seen_subcommand_from record list summary timeseries' \
    -a summary    -d 'Summarise outcomes over a time range'
complete -c xg -n '__xg_using_cmd outcomes; and not __fish_seen_subcommand_from record list summary timeseries' \
    -a timeseries -d 'Retrieve outcome metrics as a time series'

complete -c xg -n '__xg_using_subcmd outcomes record' -l session  -d 'Session id' -r
complete -c xg -n '__xg_using_subcmd outcomes record' -l outcome  -d 'Outcome label' -r
complete -c xg -n '__xg_using_subcmd outcomes record' -l score    -d 'Numeric score' -r
complete -c xg -n '__xg_using_subcmd outcomes record' -l metadata -d 'JSON metadata blob' -r

complete -c xg -n '__xg_using_subcmd outcomes list' -l session -d 'Session id' -r
complete -c xg -n '__xg_using_subcmd outcomes list' -l tenant  -d 'Tenant id' -r
complete -c xg -n '__xg_using_subcmd outcomes list' -l limit   -d 'Max results' -r
complete -c xg -n '__xg_using_subcmd outcomes list' -l offset  -d 'Pagination offset' -r
complete -c xg -n '__xg_using_subcmd outcomes list' -l output  -d 'Output format' -r -a 'table json yaml'

complete -c xg -n '__xg_using_subcmd outcomes summary' -l tenant -d 'Tenant id' -r
complete -c xg -n '__xg_using_subcmd outcomes summary' -l from   -d 'Start time (ISO-8601)' -r
complete -c xg -n '__xg_using_subcmd outcomes summary' -l to     -d 'End time (ISO-8601)' -r
complete -c xg -n '__xg_using_subcmd outcomes summary' -l output -d 'Output format' -r -a 'table json yaml'

complete -c xg -n '__xg_using_subcmd outcomes timeseries' -l tenant  -d 'Tenant id' -r
complete -c xg -n '__xg_using_subcmd outcomes timeseries' -l metric  -d 'Metric name' -r
complete -c xg -n '__xg_using_subcmd outcomes timeseries' -l from    -d 'Start time (ISO-8601)' -r
complete -c xg -n '__xg_using_subcmd outcomes timeseries' -l to      -d 'End time (ISO-8601)' -r
complete -c xg -n '__xg_using_subcmd outcomes timeseries' -l bucket  -d 'Bucket size (e.g. 1h)' -r
complete -c xg -n '__xg_using_subcmd outcomes timeseries' -l output  -d 'Output format' -r -a 'table json yaml'

# ── skills (wave-3) ──────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd skills; and not __fish_seen_subcommand_from list install' \
    -a list    -d 'List skill packs'
complete -c xg -n '__xg_using_cmd skills; and not __fish_seen_subcommand_from list install' \
    -a install -d 'Install a skill pack'

complete -c xg -n '__xg_using_subcmd skills list' -l installed  -d 'Show only installed packs'
complete -c xg -n '__xg_using_subcmd skills list' -l available  -d 'Show only available (catalog) packs'
complete -c xg -n '__xg_using_subcmd skills list' -l output     -d 'Output format' -r -a 'table json yaml'

complete -c xg -n '__xg_using_subcmd skills install' -l name    -d 'Pack name' -r
complete -c xg -n '__xg_using_subcmd skills install' -l version -d 'Pack version' -r
complete -c xg -n '__xg_using_subcmd skills install' -l force   -d 'Force reinstall'

# ── watch (wave-3) ───────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd watch; and not __fish_seen_subcommand_from list start stop test' \
    -a list  -d 'List active watch rules'
complete -c xg -n '__xg_using_cmd watch; and not __fish_seen_subcommand_from list start stop test' \
    -a start -d 'Start a watch rule from a spec file'
complete -c xg -n '__xg_using_cmd watch; and not __fish_seen_subcommand_from list start stop test' \
    -a stop  -d 'Stop a running watch rule'
complete -c xg -n '__xg_using_cmd watch; and not __fish_seen_subcommand_from list start stop test' \
    -a test  -d 'Test a watch rule without side effects'

complete -c xg -n '__xg_using_subcmd watch list' -l output -d 'Output format' -r -a 'table json yaml'

complete -c xg -n '__xg_using_subcmd watch start' -l name -d 'Watch rule name' -r
complete -c xg -n '__xg_using_subcmd watch start' -l spec -d 'Path to spec YAML' -r -F

complete -c xg -n '__xg_using_subcmd watch stop' -l name -d 'Watch rule name' -r

complete -c xg -n '__xg_using_subcmd watch test' -l name    -d 'Watch rule name' -r
complete -c xg -n '__xg_using_subcmd watch test' -l dry-run -d 'Preview without side effects'
complete -c xg -n '__xg_using_subcmd watch test' -l output  -d 'Output format' -r -a 'table json yaml'

# ── anomaly (wave-3) ──────────────────────────────────────────────────────────

complete -c xg -n '__xg_using_cmd anomaly; and not __fish_seen_subcommand_from run test' \
    -a run  -d 'Run anomaly detection against a data source'
complete -c xg -n '__xg_using_cmd anomaly; and not __fish_seen_subcommand_from run test' \
    -a test -d 'Test anomaly detection spec without persisting results'

complete -c xg -n '__xg_using_subcmd anomaly run' -l spec      -d 'Path to anomaly spec YAML' -r -F
complete -c xg -n '__xg_using_subcmd anomaly run' -l source    -d 'Data source identifier' -r
complete -c xg -n '__xg_using_subcmd anomaly run' -l window    -d 'Detection window (e.g. 24h)' -r
complete -c xg -n '__xg_using_subcmd anomaly run' -l threshold -d 'Alert threshold (0.0-1.0)' -r
complete -c xg -n '__xg_using_subcmd anomaly run' -l output    -d 'Output format' -r -a 'table json yaml'

complete -c xg -n '__xg_using_subcmd anomaly test' -l spec    -d 'Path to anomaly spec YAML' -r -F
complete -c xg -n '__xg_using_subcmd anomaly test' -l dry-run -d 'Preview detections without persisting'
complete -c xg -n '__xg_using_subcmd anomaly test' -l output  -d 'Output format' -r -a 'table json yaml'

# Mirror all completions for the "xiaoguai" binary name
# (fish doesn't support alias completion natively; we duplicate the file at install time)
