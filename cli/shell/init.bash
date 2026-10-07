# ctrl+shift+r: pick a contextleleo session recorded in the current folder.
contextleleo_cwd_picker() {
  printf '\e[=0;1u'                       # legacy keys for the picker's raw reader
  contextleleo query --cwd "$PWD" < /dev/tty
  printf '\e[=1;1u'
}
case $TERM in (*kitty*|*ghostty*|*wezterm*)
  # In legacy encoding ctrl+shift+r and ctrl+r send the same byte. Enable the
  # kitty keyboard protocol (flag 1, "disambiguate escape codes") only while
  # readline reads a line: on when the prompt is shown, off just before the
  # command runs.
  PROMPT_COMMAND="printf '\\e[=1;1u'${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
  PS0="\$(printf '\\e[=0;1u')${PS0-}"
  # Under flag 1, every modified key arrives as a CSI-u sequence; feed the
  # legacy bytes back so every existing binding still fires.
  _contextleleo_letters=abcdefghijklmnopqrstuvwxyz
  for ((_contextleleo_i = 0; _contextleleo_i < 26; _contextleleo_i++)); do
    _contextleleo_l=${_contextleleo_letters:_contextleleo_i:1}
    _contextleleo_k=$((97 + _contextleleo_i))
    bind "\"\e[${_contextleleo_k};5u\": \"\C-${_contextleleo_l}\""     # ctrl+letter
    bind "\"\e[${_contextleleo_k};7u\": \"\e\C-${_contextleleo_l}\""   # ctrl+alt+letter
  done
  for ((_contextleleo_k = 33; _contextleleo_k <= 126; _contextleleo_k++)); do
    printf -v _contextleleo_l "\\$(printf '%03o' "$_contextleleo_k")"
    case $_contextleleo_l in ('"'|'\') _contextleleo_l=\\$_contextleleo_l ;; esac
    bind "\"\e[${_contextleleo_k};3u\": \"\e${_contextleleo_l}\""      # alt+printable
  done
  unset _contextleleo_letters _contextleleo_i _contextleleo_l _contextleleo_k
  bind '"\e[32;3u": "\e "'                                   # alt+space
  bind '"\e[127;3u": "\e\C-?"'                               # alt+backspace: kill word
  bind '"\e[127;5u": "\C-h"'                                 # ctrl+backspace
  bind '"\e[127;7u": "\e\C-h"'                               # ctrl+alt+backspace
  bind '"\e[13;2u": "\C-m"'                                  # shift+enter
  bind '"\e[13;3u": "\e\C-m"'                                # alt+enter
  bind '"\e[13;5u": "\C-m"'                                  # ctrl+enter
  bind '"\e[99;5u": abort'                                   # ctrl+c
  bind '"\e[27u": "\e"'                                      # bare ESC
  bind -x '"\e[114;6u": contextleleo_cwd_picker'                  # ctrl+shift+r
esac
