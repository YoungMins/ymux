import { describe, it, expect } from "vitest";
import { quotePathForShell, shellFamilyFromExecutable } from "./shellQuote";

describe("shellFamilyFromExecutable", () => {
  it("classifies by file stem like the backend's ShellFamily (plus fish)", () => {
    expect(shellFamilyFromExecutable("/bin/zsh")).toBe("posix");
    expect(shellFamilyFromExecutable("/opt/homebrew/bin/bash")).toBe("posix");
    expect(shellFamilyFromExecutable("C:\\Program Files\\Git\\bin\\bash.exe")).toBe("posix");
    expect(shellFamilyFromExecutable("pwsh.exe")).toBe("powershell");
    expect(
      shellFamilyFromExecutable("C:\\WINDOWS\\System32\\WindowsPowerShell\\v1.0\\PowerShell.EXE"),
    ).toBe("powershell");
    expect(shellFamilyFromExecutable("C:\\Windows\\system32\\cmd.exe")).toBe("cmd");
    expect(shellFamilyFromExecutable("/opt/homebrew/bin/fish")).toBe("fish");
    expect(shellFamilyFromExecutable("wsl.exe")).toBe("unknown");
    expect(shellFamilyFromExecutable("C:\\Program Files\\nu\\bin\\nu.exe")).toBe("unknown");
    expect(shellFamilyFromExecutable("/bin/tcsh")).toBe("unknown");
    expect(shellFamilyFromExecutable("")).toBe("unknown");
    expect(shellFamilyFromExecutable(undefined)).toBe("unknown");
  });
});

describe("quotePathForShell", () => {
  const nasty = [
    "/Users/me/My Files/a.txt",
    "/tmp/$HOME/x",
    "/tmp/`whoami`.png",
    "/tmp/it's here",
    "/tmp/%PATH%/y",
    "/Users/me/한글 폴더/사진.png",
    "/tmp/a\\b",
    "/tmp/!!",
  ];

  it("posix: single quotes, nothing inside expands, ' becomes '\\''", () => {
    expect(quotePathForShell("/Users/me/My Files/a.txt", "posix")).toBe(
      "'/Users/me/My Files/a.txt'",
    );
    expect(quotePathForShell("/tmp/$HOME/x", "posix")).toBe("'/tmp/$HOME/x'");
    expect(quotePathForShell("/tmp/`whoami`.png", "posix")).toBe("'/tmp/`whoami`.png'");
    expect(quotePathForShell("/tmp/it's here", "posix")).toBe("'/tmp/it'\\''s here'");
    expect(quotePathForShell("/tmp/%PATH%/y", "posix")).toBe("'/tmp/%PATH%/y'");
    expect(quotePathForShell("/Users/me/한글 폴더/사진.png", "posix")).toBe(
      "'/Users/me/한글 폴더/사진.png'",
    );
    expect(quotePathForShell("C:\\Users\\a b\\x.png", "posix")).toBe("'C:\\Users\\a b\\x.png'");
  });

  it("posix quoting round-trips through a POSIX single-quote parser", () => {
    // Minimal model of sh's quoting: '…' is literal, \x outside quotes is x,
    // and anything else unquoted would be open to expansion.
    const unquote = (s: string): string => {
      let out = "";
      let i = 0;
      while (i < s.length) {
        if (s[i] === "'") {
          const end = s.indexOf("'", i + 1);
          if (end < 0) throw new Error(`unterminated quote in ${s}`);
          out += s.slice(i + 1, end);
          i = end + 1;
        } else if (s[i] === "\\") {
          out += s[i + 1];
          i += 2;
        } else {
          throw new Error(`unquoted ${JSON.stringify(s[i])} in ${s}`);
        }
      }
      return out;
    };
    for (const p of nasty) expect(unquote(quotePathForShell(p, "posix")!)).toBe(p);
  });

  it("fish: \\ and ' are escaped inside single quotes, so nothing breaks out", () => {
    // fish's single-quote grammar: inside '…', \\ is \, \' is ', and any
    // other backslash is literal. Nothing may sit outside the one quoted run.
    const unquoteFish = (s: string): string => {
      if (s.length < 2 || s[0] !== "'") throw new Error(`not quoted: ${s}`);
      let out = "";
      let i = 1;
      for (;;) {
        if (i >= s.length) throw new Error(`unterminated: ${s}`);
        const c = s[i];
        if (c === "\\" && (s[i + 1] === "\\" || s[i + 1] === "'")) {
          out += s[i + 1];
          i += 2;
        } else if (c === "'") {
          if (i !== s.length - 1) throw new Error(`text after the closing quote: ${s}`);
          return out;
        } else {
          out += c;
          i += 1;
        }
      }
    };
    const breakout = "x\\';touch pwned;#";
    expect(quotePathForShell(breakout, "fish")).toBe("'x\\\\\\';touch pwned;#'");
    for (const p of [...nasty, breakout, "/tmp/trailing\\", "C:\\Users\\a b"]) {
      expect(unquoteFish(quotePathForShell(p, "fish")!)).toBe(p);
    }
    // The POSIX form of the same name breaks out of fish's quotes.
    expect(() => unquoteFish(quotePathForShell(breakout, "posix")!)).toThrow();
  });

  it("powershell: single quotes, quotes doubled (smart quotes too)", () => {
    expect(quotePathForShell("C:\\Users\\a b\\$env:PATH\\x", "powershell")).toBe(
      "'C:\\Users\\a b\\$env:PATH\\x'",
    );
    expect(quotePathForShell("C:\\tmp\\$(calc)", "powershell")).toBe("'C:\\tmp\\$(calc)'");
    expect(quotePathForShell("C:\\tmp\\`a", "powershell")).toBe("'C:\\tmp\\`a'");
    expect(quotePathForShell("C:\\it's", "powershell")).toBe("'C:\\it''s'");
    expect(quotePathForShell("C:\\it\u2019s", "powershell")).toBe("'C:\\it\u2019\u2019s'");
    expect(quotePathForShell("C:\\%PATH%\\x", "powershell")).toBe("'C:\\%PATH%\\x'");
    expect(quotePathForShell("C:\\사진\\a.png", "powershell")).toBe("'C:\\사진\\a.png'");
  });

  it('cmd: double quotes (no $ expansion; Windows forbids " in names)', () => {
    expect(quotePathForShell("C:\\Users\\John Smith\\clip.png", "cmd")).toBe(
      '"C:\\Users\\John Smith\\clip.png"',
    );
    expect(quotePathForShell("C:\\$HOME\\`x`\\it's", "cmd")).toBe('"C:\\$HOME\\`x`\\it\'s"');
    // Documented limitation: %VAR% cannot be escaped on cmd's interactive line.
    expect(quotePathForShell("C:\\%PATH%\\x", "cmd")).toBe('"C:\\%PATH%\\x"');
  });

  it("unknown (WSL, nu, csh, custom): plain single quotes only when nothing inside could be special", () => {
    expect(quotePathForShell("/mnt/c/Users/me/$(calc.exe).png", "unknown")).toBe(
      "'/mnt/c/Users/me/$(calc.exe).png'",
    );
    expect(quotePathForShell("/tmp/`whoami`.png", "unknown")).toBe("'/tmp/`whoami`.png'");
    expect(quotePathForShell("/Users/me/한글 폴더/a.png", "unknown")).toBe(
      "'/Users/me/한글 폴더/a.png'",
    );
    for (const p of [
      "/tmp/it's",
      "/tmp/it\u2019s",
      "/tmp/a\\b",
      "/tmp/!!",
      "C:\\Users\\me\\$(calc.exe).png",
      "C:\\a b",
    ]) {
      expect(quotePathForShell(p, "unknown"), p).toBeNull();
    }
  });

  it("refuses empty paths and paths with control characters", () => {
    for (const family of ["posix", "fish", "powershell", "cmd", "unknown"] as const) {
      expect(quotePathForShell("", family)).toBeNull();
      expect(quotePathForShell("/tmp/x\nrm -rf ~", family)).toBeNull();
      expect(quotePathForShell("/tmp/x\ry", family)).toBeNull();
      expect(quotePathForShell("/tmp/\x1b[201~", family)).toBeNull();
    }
  });
});
