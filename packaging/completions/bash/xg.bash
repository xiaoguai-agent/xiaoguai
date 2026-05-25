# bash completion for xg (xiaoguai CLI)
# Installation:
#   source /path/to/xg.bash          # temporary (current session)
#   echo 'source /path/to/xg.bash' >> ~/.bashrc   # permanent
#
# Also works if the binary is invoked as "xiaoguai".

_xg_completions() {
    local cur prev words cword
    _init_completion || return

    # Top-level subcommands (wave-3 commands included)
    local top_cmds="chat provider mcp remote eval backup restore self-update audit hotl outcomes skills watch anomaly"

    # Common global flags
    local global_flags="--config --help --version"

    # Output format flag values
    local output_formats="table json yaml"

    case "${words[1]}" in
        chat)
            case "$prev" in
                --prompt|--ollama-url|--model) return ;;
            esac
            COMPREPLY=($(compgen -W "--prompt --mock --ollama-url --model $global_flags" -- "$cur"))
            return
            ;;

        provider)
            local provider_cmds="register list remove"
            case "${words[2]}" in
                register)
                    COMPREPLY=($(compgen -W "--name --kind --endpoint --models --default-for --fallback-order --api-key-env --tenant $global_flags" -- "$cur"))
                    ;;
                list)
                    COMPREPLY=($(compgen -W "--tenant $global_flags" -- "$cur"))
                    ;;
                remove)
                    COMPREPLY=($(compgen -W "--id $global_flags" -- "$cur"))
                    ;;
                *)
                    COMPREPLY=($(compgen -W "$provider_cmds" -- "$cur"))
                    ;;
            esac
            return
            ;;

        mcp)
            local mcp_cmds="register list remove"
            case "${words[2]}" in
                register)
                    COMPREPLY=($(compgen -W "--name --version --transport --command --args --env-keys --endpoint --tenant $global_flags" -- "$cur"))
                    ;;
                list)
                    COMPREPLY=($(compgen -W "--tenant $global_flags" -- "$cur"))
                    ;;
                remove)
                    COMPREPLY=($(compgen -W "--id $global_flags" -- "$cur"))
                    ;;
                *)
                    COMPREPLY=($(compgen -W "$mcp_cmds" -- "$cur"))
                    ;;
            esac
            return
            ;;

        remote)
            local remote_cmds="healthz chat messages cancel"
            case "${words[2]}" in
                healthz)
                    COMPREPLY=($(compgen -W "--server $global_flags" -- "$cur"))
                    ;;
                chat)
                    COMPREPLY=($(compgen -W "--server --user-id --tenant-id --model --prompt --title $global_flags" -- "$cur"))
                    ;;
                messages)
                    COMPREPLY=($(compgen -W "--server --session $global_flags" -- "$cur"))
                    ;;
                cancel)
                    COMPREPLY=($(compgen -W "--server --session $global_flags" -- "$cur"))
                    ;;
                *)
                    COMPREPLY=($(compgen -W "$remote_cmds" -- "$cur"))
                    ;;
            esac
            return
            ;;

        eval)
            local eval_cmds="run"
            case "${words[2]}" in
                run)
                    COMPREPLY=($(compgen -W "--suite --cases-dir --out --max-iterations $global_flags" -- "$cur"))
                    ;;
                *)
                    COMPREPLY=($(compgen -W "$eval_cmds" -- "$cur"))
                    ;;
            esac
            return
            ;;

        backup)
            case "$prev" in
                --out|--encrypt) _filedir; return ;;
            esac
            COMPREPLY=($(compgen -W "--out --database-url --encrypt $global_flags" -- "$cur"))
            return
            ;;

        restore)
            case "$prev" in
                --in|--identity) _filedir; return ;;
                --outdir) _filedir -d; return ;;
            esac
            COMPREPLY=($(compgen -W "--in --outdir --force --identity $global_flags" -- "$cur"))
            return
            ;;

        self-update)
            COMPREPLY=($(compgen -W "--check $global_flags" -- "$cur"))
            return
            ;;

        audit)
            local audit_cmds="export"
            case "${words[2]}" in
                export)
                    COMPREPLY=($(compgen -W "--sink --bucket --prefix --region --endpoint-url --sink-name --interval-secs --once --database-url $global_flags" -- "$cur"))
                    ;;
                *)
                    COMPREPLY=($(compgen -W "$audit_cmds" -- "$cur"))
                    ;;
            esac
            return
            ;;

        # Wave-3: hotl policy management
        hotl)
            local hotl_cmds="policy check"
            case "${words[2]}" in
                policy)
                    local policy_cmds="create list get update delete"
                    case "${words[3]}" in
                        create)
                            COMPREPLY=($(compgen -W "--name --rule --priority --enabled --config $global_flags" -- "$cur"))
                            ;;
                        list)
                            COMPREPLY=($(compgen -W "--output --limit --offset --config $global_flags" -- "$cur"))
                            if [[ "$prev" == "--output" ]]; then
                                COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                            fi
                            ;;
                        get)
                            COMPREPLY=($(compgen -W "--id --output --config $global_flags" -- "$cur"))
                            if [[ "$prev" == "--output" ]]; then
                                COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                            fi
                            ;;
                        update)
                            COMPREPLY=($(compgen -W "--id --name --rule --priority --enabled --config $global_flags" -- "$cur"))
                            ;;
                        delete)
                            COMPREPLY=($(compgen -W "--id --force --config $global_flags" -- "$cur"))
                            ;;
                        *)
                            COMPREPLY=($(compgen -W "$policy_cmds" -- "$cur"))
                            ;;
                    esac
                    ;;
                check)
                    COMPREPLY=($(compgen -W "--tenant --session --output --config $global_flags" -- "$cur"))
                    if [[ "$prev" == "--output" ]]; then
                        COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                    fi
                    ;;
                *)
                    COMPREPLY=($(compgen -W "$hotl_cmds" -- "$cur"))
                    ;;
            esac
            return
            ;;

        # Wave-3: outcome telemetry
        outcomes)
            local outcomes_cmds="record list summary timeseries"
            case "${words[2]}" in
                record)
                    COMPREPLY=($(compgen -W "--session --outcome --score --metadata --config $global_flags" -- "$cur"))
                    ;;
                list)
                    COMPREPLY=($(compgen -W "--session --tenant --limit --offset --output --config $global_flags" -- "$cur"))
                    if [[ "$prev" == "--output" ]]; then
                        COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                    fi
                    ;;
                summary)
                    COMPREPLY=($(compgen -W "--tenant --from --to --output --config $global_flags" -- "$cur"))
                    if [[ "$prev" == "--output" ]]; then
                        COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                    fi
                    ;;
                timeseries)
                    COMPREPLY=($(compgen -W "--tenant --metric --from --to --bucket --output --config $global_flags" -- "$cur"))
                    if [[ "$prev" == "--output" ]]; then
                        COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                    fi
                    ;;
                *)
                    COMPREPLY=($(compgen -W "$outcomes_cmds" -- "$cur"))
                    ;;
            esac
            return
            ;;

        # Wave-3: skill pack management
        skills)
            local skills_cmds="list install"
            case "${words[2]}" in
                list)
                    COMPREPLY=($(compgen -W "--installed --available --output --config $global_flags" -- "$cur"))
                    if [[ "$prev" == "--output" ]]; then
                        COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                    fi
                    ;;
                install)
                    COMPREPLY=($(compgen -W "--name --version --force --config $global_flags" -- "$cur"))
                    ;;
                *)
                    COMPREPLY=($(compgen -W "$skills_cmds" -- "$cur"))
                    ;;
            esac
            return
            ;;

        # Wave-3: watch rules
        watch)
            local watch_cmds="list start stop test"
            case "${words[2]}" in
                list)
                    COMPREPLY=($(compgen -W "--output --config $global_flags" -- "$cur"))
                    if [[ "$prev" == "--output" ]]; then
                        COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                    fi
                    ;;
                start)
                    COMPREPLY=($(compgen -W "--name --spec --config $global_flags" -- "$cur"))
                    if [[ "$prev" == "--spec" ]]; then _filedir yaml; fi
                    ;;
                stop)
                    COMPREPLY=($(compgen -W "--name --config $global_flags" -- "$cur"))
                    ;;
                test)
                    COMPREPLY=($(compgen -W "--name --dry-run --output --config $global_flags" -- "$cur"))
                    if [[ "$prev" == "--output" ]]; then
                        COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                    fi
                    ;;
                *)
                    COMPREPLY=($(compgen -W "$watch_cmds" -- "$cur"))
                    ;;
            esac
            return
            ;;

        # Wave-3: anomaly detection
        anomaly)
            local anomaly_cmds="run test"
            case "${words[2]}" in
                run)
                    COMPREPLY=($(compgen -W "--spec --source --window --threshold --output --config $global_flags" -- "$cur"))
                    if [[ "$prev" == "--spec" ]]; then _filedir yaml; fi
                    if [[ "$prev" == "--output" ]]; then
                        COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                    fi
                    ;;
                test)
                    COMPREPLY=($(compgen -W "--spec --dry-run --output --config $global_flags" -- "$cur"))
                    if [[ "$prev" == "--spec" ]]; then _filedir yaml; fi
                    if [[ "$prev" == "--output" ]]; then
                        COMPREPLY=($(compgen -W "$output_formats" -- "$cur"))
                    fi
                    ;;
                *)
                    COMPREPLY=($(compgen -W "$anomaly_cmds" -- "$cur"))
                    ;;
            esac
            return
            ;;
    esac

    # Top-level: no subcommand yet (or partial)
    if [[ "$cword" -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$top_cmds $global_flags" -- "$cur"))
    fi
}

complete -F _xg_completions xg
complete -F _xg_completions xiaoguai
