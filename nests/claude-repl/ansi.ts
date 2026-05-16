// ANSI/CSI stripper covering the private-mode and OSC variants Ink emits.

export function stripAnsi(text: string): string {
  return text
    // Standard + private-mode CSIs:
    //   \x1b[1;31m   (SGR — standard)
    //   \x1b[?25h    (DECSET — `?` private-marker, ends `h`)
    //   \x1b[?25l    (DECRST — ends `l`)
    //   \x1b[>0q     (XTVERSION query — `>` private-marker, ends `q`)
    //   \x1b[18t     (XTWINOPS — `t` is a-z so the standard branch
    //                 above would already match; double-covered here)
    // The `[?>]?` matches optional private-marker after `[`.
    .replace(/\x1b\[[?>]?[0-9;]*[a-zA-Z]/g, "")
    .replace(/\x1b\][0-9;]*[^\x1b]*(\x1b\\|\x07)/g, "")
    .replace(/\x1b[[\](][0-9;]*[a-zA-Z]/g, "")
    .replace(/\x1b[PX^_].*?\x1b\\/g, "")
    .replace(/\x1b\[[?>]?[0-9;]*[Hf]/g, "\n")
    .replace(/\x1b\[[?>]?[0-9]*[JKl]/g, "")
    .replace(/\r/g, "")
    .replace(/\n{4,}/g, "\n\n")
    .trim();
}
