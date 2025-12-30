app [main!] { pf: platform "../platform/main.roc" }

import pf.Dir
import pf.Stdout

main! : List(Str) => Try({}, [Exit(I32)])
main! = |_args| {
    # Create a single directory
    Dir.create!("test-dir")
    Stdout.line!("Created test-dir")

    # Create nested directories
    Dir.create_all!("test-nested/a/b/c")
    Stdout.line!("Created test-nested/a/b/c")

    # List directory contents
    entries = Dir.list!("test-nested")
    Stdout.line!("Contents: ${Str.join_with(entries, ", ")}")

    # Clean up
    Dir.delete_empty!("test-dir")
    Dir.delete_all!("test-nested")
    Stdout.line!("Cleaned up!")

    Ok({})
}
