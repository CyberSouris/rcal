#!/usr/bin/env bash
#
# Bash completion for rcal
#
# To use, source this file in your shell:
#   source completions/rcal.bash
#
# Or copy it to a location on your fpath, e.g.:
#   cp completions/rcal.bash /etc/bash_completion.d/rcal

_rcal() {
    local cur prev words cword
    _init_completion || return

    # Find the subcommand (first non-flag argument after "rcal")
    local subcmd=""
    local i
    for ((i = 1; i < cword; i++)); do
        case "${words[i]}" in
            today|day|week|month|show|import|sync|new|search|calendars|init)
                subcmd="${words[i]}"
                if [[ "${words[i]}" == "day" ]]; then
                    subcmd="today"
                fi
                break
                ;;
            -*)
                continue
                ;;
        esac
    done

    # If we're completing a flag value, handle per-subcommand cases
    if [[ "$cur" == -* ]] || [[ ${prev} == -* ]]; then
        # Completing a flag value for known options
        case "$subcmd" in
            today)
                case "$prev" in
                    -d|--date)
                        _rcal_date_completion
                        return
                        ;;
                esac
                ;;
            week)
                case "$prev" in
                    -d|--date)
                        _rcal_date_completion
                        return
                        ;;
                esac
                ;;
            month)
                case "$prev" in
                    -m|--month)
                        COMPREPLY=($(compgen -W "$(_rcal_months)" -- "$cur"))
                        return
                        ;;
                esac
                ;;
            show)
                case "$prev" in
                    date)
                        _rcal_date_completion
                        return
                        ;;
                esac
                ;;
            new)
                case "$prev" in
                    -d|--date)
                        _rcal_date_completion
                        return
                        ;;
                    -t|--time)
                        COMPREPLY=($(compgen -W "00:00 01:00 02:00 03:00 04:00 05:00 06:00 07:00 08:00 09:00 10:00 11:00 12:00 13:00 14:00 15:00 16:00 17:00 18:00 19:00 20:00 21:00 22:00 23:00" -- "$cur"))
                        return
                        ;;
                    -D|--duration)
                        COMPREPLY=($(compgen -W "15 30 45 60 90 120 180 240 480 720 1440" -- "$cur"))
                        return
                        ;;
                    -c|--calendar)
                        _rcal_calendar_completion
                        return
                        ;;
                esac
                ;;
            search)
                case "$prev" in
                    -f|--from|-t|--to)
                        _rcal_date_completion
                        return
                        ;;
                esac
                ;;
            init)
                case "$prev" in
                    -u|--url)
                        COMPREPLY=($(compgen -W "http:// https://" -- "$cur"))
                        return
                        ;;
                    --password_command)
                        COMPREPLY=($(compgen -c -- "$cur"))
                        return
                        ;;
                esac
                ;;
        esac

        # If currently typing a flag, complete flags for the current subcommand
        if [[ "$cur" == -* ]]; then
            _rcal_flag_completion "$subcmd"
            return
        fi
    fi

    # Completing a positional argument
    case "$subcmd" in
        show)
            _rcal_date_completion
            return
            ;;
        import)
            _filedir '@(ics|ical|ifb|icalendar)'
            return
            ;;
        search)
            # First positional is the query — no special completion
            return
            ;;
    esac

    # No subcommand yet — complete subcommands and global flags
    local subcommands="today day week month show import sync new search calendars init"
    local global_flags="--version --help -h -V"
    COMPREPLY=($(compgen -W "$subcommands $global_flags" -- "$cur"))
}

_rcal_flag_completion() {
    local subcmd="$1"
    local cur="$2"
    local flags=""

    case "$subcmd" in
        today)      flags="--details --date --next --help -d -h" ;;
        week)       flags="--date --next --agenda --details --help -d -h" ;;
        month)      flags="--month --next --agenda --details --help -m -h" ;;
        show)       flags="--details --help -h" ;;
        import)     flags="--add --dry-run --help -h --no-interactive" ;;
        sync)       flags="--help -h" ;;
        new)        flags="--title --date --time --duration --all_day --location --description --calendar --help -t -d -D -l -c -a -h" ;;
        search)     flags="--from --to --help -f -t -h" ;;
        calendars)  flags="--help -h" ;;
        init)       flags="--url --username --password_command --force --help -u -f -h" ;;
    esac

    COMPREPLY=($(compgen -W "$flags" -- "$cur"))
}

_rcal_date_completion() {
    # Suggest recent and upcoming dates around today
    local today
    today=$(date +%Y-%m-%d 2>/dev/null)
    if [[ -n "$today" ]]; then
        local dates=""
        local d
        for offset in -7 -3 -2 -1 0 1 2 3 7 14 30; do
            d=$(date -d "$today + $offset days" +%Y-%m-%d 2>/dev/null)
            if [[ -n "$d" ]]; then
                dates="$dates $d"
            fi
        done
        COMPREPLY=($(compgen -W "$dates" -- "$cur"))
    fi
}

_rcal_months() {
    # Suggest YYYY-MM for current and nearby months
    local now
    now=$(date +%Y-%m 2>/dev/null)
    if [[ -n "$now" ]]; then
        local months=""
        local m
        for offset in -3 -2 -1 0 1 2 3; do
            m=$(date -d "$(date +%Y-%m-01) + $offset months" +%Y-%m 2>/dev/null)
            if [[ -n "$m" ]]; then
                months="$months $m"
            fi
        done
        echo "$months"
    fi
}

_rcal_calendar_completion() {
    # Try to list calendars from rcal if the binary is available
    if command -v rcal &>/dev/null; then
        local calendars
        calendars=$(rcal calendars 2>/dev/null | grep -oP '\S+' | tail -n +2)
        if [[ -n "$calendars" ]]; then
            COMPREPLY=($(compgen -W "$calendars" -- "$cur"))
            return
        fi
    fi
}

complete -F _rcal rcal
