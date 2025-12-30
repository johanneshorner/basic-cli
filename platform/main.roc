platform ""
    requires {} { main! : List(Str) => Try({}, [Exit(I32), ..others]) }
    exposes [Dir, Env, File, Path, Stdin, Stdout, Stderr]
    packages {}
    provides { main_for_host! : "main_for_host" }
    targets: {
        files: "targets/",
        exe: {
            x64mac: ["libhost.a", app],
            arm64mac: ["libhost.a", app],
            x64musl: ["crt1.o", "libhost.a", "libunwind.a", app, "libc.a"],
            arm64musl: ["crt1.o", "libhost.a", "libunwind.a", app, "libc.a"],
        }
    }

import Dir
import Env
import File
import Path
import Stdin
import Stdout
import Stderr

main_for_host! : List(Str) => I32
main_for_host! = |args|
    match main!(args) {
        Ok({}) => 0
        Err(Exit(code)) => code
        Err(other) => {
            Stderr.line!("Program exited with error: ${Str.inspect(other)}")
            1
        }
    }
