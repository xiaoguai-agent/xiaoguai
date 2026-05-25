# Shell Completions for `xg` / `xiaoguai`

Hand-written completions covering the full `xg` subcommand tree, including wave-3
commands (`hotl`, `outcomes`, `skills`, `watch`, `anomaly`).  The completions also
register under the canonical binary name `xiaoguai`.

> **Approach**: hand-written against the clap source tree.  The binary already
> ships a hidden `xg completions [bash|zsh|fish|pwsh|elvish]` subcommand (backed
> by `clap_complete`) for generated completions, but the wave-3 subcommands are
> not yet wired into that code path.  These static files bridge the gap and serve
> as the packaging-time reference until the Rust source is updated.

---

## Bash

```sh
# Option 1 — source for the current session only
source /usr/share/xg/completions/bash/xg.bash

# Option 2 — persist (add to ~/.bashrc or ~/.bash_profile)
echo 'source /usr/share/xg/completions/bash/xg.bash' >> ~/.bashrc

# Option 3 — drop into bash-completion.d (picked up automatically on Debian/Ubuntu)
sudo cp packaging/completions/bash/xg.bash /etc/bash_completion.d/xg
```

## Zsh

```sh
# Create a custom fpath directory (once)
mkdir -p ~/.zfunc
echo 'fpath=(~/.zfunc $fpath)' >> ~/.zshrc
echo 'autoload -Uz compinit && compinit' >> ~/.zshrc

# Copy the completion function
cp packaging/completions/zsh/_xg ~/.zfunc/_xg

# Reload your shell or run:
exec zsh
```

If you manage completions with a framework (Oh My Zsh, Zinit, etc.) place `_xg`
in any directory already on your `$fpath`.

## Fish

```sh
# Per-user installation
cp packaging/completions/fish/xg.fish ~/.config/fish/completions/xg.fish

# Also install under the canonical binary name if needed
cp packaging/completions/fish/xg.fish ~/.config/fish/completions/xiaoguai.fish

# Reload (fish picks up new completion files automatically in a new session)
```

---

## Homebrew formula hint

Add the following stanza to your Homebrew formula to install completions
automatically:

```ruby
def install
  bin.install "xg"

  # Shell completions
  bash_completion.install "packaging/completions/bash/xg.bash" => "xg"
  zsh_completion.install  "packaging/completions/zsh/_xg"
  fish_completion.install "packaging/completions/fish/xg.fish"
end
```

---

## Coverage

| Shell | File | Depth |
|-------|------|-------|
| Bash  | `bash/xg.bash` | top-level + all subcommands + wave-3 (depth 3 for `hotl policy <action>`) |
| Zsh   | `zsh/_xg` | same, with argument descriptions |
| Fish  | `fish/xg.fish` | same, structured as `complete -c xg -n '...'` rules |

### Subcommands covered

**Existing**
- `chat` — `--prompt`, `--mock`, `--ollama-url`, `--model`
- `provider {register,list,remove}`
- `mcp {register,list,remove}`
- `remote {healthz,chat,messages,cancel}`
- `eval run`
- `backup` / `restore`
- `self-update`
- `audit export`

**Wave-3**
- `hotl policy {create,list,get,update,delete}`
- `hotl check`
- `outcomes {record,list,summary,timeseries}`
- `skills {list,install}`
- `watch {list,start,stop,test}`
- `anomaly {run,test}`

**Global flag**: `--config` (all commands)
