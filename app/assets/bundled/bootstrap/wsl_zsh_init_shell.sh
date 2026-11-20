# Each line starts with a leading space to leave these commands out of history.
#
# command -p resolves the given command with the system default PATH, ensuring the shell
# can find them even if the user has a clobbered PATH value.
 unsetopt ZLE
 WARP_SESSION_ID=@@WARP_SESSION_ID@@
 _hostname=$(command -pv hostname >/dev/null 2>&1 && command -p hostname 2>/dev/null || command -p uname -n)
 _user=$(command -pv whoami >/dev/null 2>&1 && command -p whoami 2>/dev/null || echo $USER)
 WARP_USING_WINDOWS_CON_PTY=@@USING_CON_PTY_BOOLEAN@@
 # Tell NeoWarp to omit the streamed bootstrap only after opening the file.
 unset _warp_bootstrap_fd
 _warp_from_file=false
 [[ "$WARP_USING_WINDOWS_CON_PTY" == true && -n "${WSL_DISTRO_NAME:-}" ]] || unset _warp_bootstrap_path
 if [[ -f "${_warp_bootstrap_path:-}" ]]; then { exec {_warp_bootstrap_fd}<"$_warp_bootstrap_path"; } 2>/dev/null; fi
 if (( ${+_warp_bootstrap_fd} )) && [[ -r "/dev/fd/$_warp_bootstrap_fd" ]]; then _warp_from_file=true; fi
 _warp_json="{\"hook\":\"InitShell\",\"value\":{\"session_id\":%s,\"shell\":\"zsh\",\"user\":\"%s\",\"hostname\":\"%s\",\"bootstrap_from_file\":%s}}"
 _msg=$(printf "$_warp_json" "$WARP_SESSION_ID" "$_user" "$_hostname" "$_warp_from_file" | command -p od -An -v -tx1 | command -p tr -d " \n")
 # We send the InitShell hook via OSCs when on Windows and via DCSs otherwise.
 if [ "$WARP_USING_WINDOWS_CON_PTY" = true ]; then printf '\e]9278;d;%s\x07' "$_msg"; else printf '\x1b\x50\x24\x64%s\x1b\x5c' "$_msg"; fi
 unset _hostname _user _msg _warp_json _warp_bootstrap_path
 if [[ "$_warp_from_file" == true ]]; then source "/dev/fd/$_warp_bootstrap_fd"; fi
 if (( ${+_warp_bootstrap_fd} )); then exec {_warp_bootstrap_fd}<&-; fi
 unset _warp_bootstrap_fd _warp_from_file
