platform ""
    requires {} { main! : List(Str) => Try({}, [Exit(I32)]) }
    exposes [Dir, Env, File, Stdin, Stdout, Stderr]
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
import Stdin
import Stdout
import Stderr

main_for_host! : List(Str) => I32
main_for_host! = |args| {
    result = main!(args)
    match result {
        Ok({}) => 0
        Err(Exit(code)) => code
        # TODO enable when open tag union supported in platform header
        #Err(err) => {
        #    err_str = Str.inspect(err)

        #    # Inspect adds parentheses around errors, which are unnecessary here.
        #    clean_err_str =
        #        if Str.starts_with(err_str, "(") and Str.ends_with(err_str, ")") {
        #            err_str
        #                .drop_prefix("(")
        #                .drop_suffix(")")
        #        } else {
        #            err_str
        #        }

        #    help_msg =
        #        \\
        #        \\Program exited with error:
        #        \\
        #        \\    ${clean_err_str}

        #    Stderr.line!(help_msg)
        #    1
        #}
    }
}
