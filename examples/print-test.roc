app [main!] { pf: platform "../platform/main.roc" }

import pf.Stdout
import pf.Stderr

main! : List(Str) => Try({}, [Exit(I32)])
main! = |_args| {
    # Test write without newline
    Stdout.write!("Hello")
    Stdout.write!(", ")
    Stdout.line!("World!")

    # Test stderr write
    Stderr.write!("Error: ")
    Stderr.line!("something happened")

    Ok({})
}
