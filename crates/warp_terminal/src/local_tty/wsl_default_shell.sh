# Resolve in a child to preserve startup overrides without inheriting its environment.
selected=$("$SHELL" -c 'printenv SHELL' </dev/null 2>/dev/null) || exit 65
selected=${selected%%'
'*}
[ -f "$selected" ] && [ -x "$selected" ] || exit 65

case "$selected" in
    *bash*)
        exec "$selected" -c 'exec -a bash "$1" --rcfile <(printf "BASH=%q;%s\n" "$1" "$2")' bash "$selected" "$1"
        ;;
    *zsh*)
        # The interactive zsh receives its existing init command from NeoWarp.
        printf '\033]9278;d;%s\007' "$3"
        exec "$selected" -c 'exec -a -zsh "$1" -g --no-rcs' zsh "$selected"
        ;;
    *fish*)
        exec "$selected" -f no-mark-prompt --login --init-command "$2"
        ;;
    *) exit 65 ;;
esac
