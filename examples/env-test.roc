app [main!] { pf: platform "../platform/main.roc" }

import pf.Env
import pf.Stdout

main! : List(Str) => Try({}, [Exit(I32)])
main! = |_args| {
    editor = Env.var!("EDITOR")
    Stdout.line!("Your editor is: ${editor}")

    cwd = Env.cwd!({})
    Stdout.line!("Current directory: ${cwd}")

    exe = Env.exe_path!({})
    Stdout.line!("Executable: ${exe}")

    Ok({})
}
