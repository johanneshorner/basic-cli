app [main!] { pf: platform "../platform/main.roc" }

import pf.File
import pf.Stdout

main! : List(Str) => Try({}, [Exit(I32)])
main! = |_args| {
    # Test write and read UTF-8
    File.write_utf8!("test-file.txt", "Hello, Roc!")
    content = File.read_utf8!("test-file.txt")
    Stdout.line!("Read: ${content}")

    # Test write and read bytes
    File.write_bytes!("test-bytes.bin", [1, 2, 3, 4, 5])
    bytes = File.read_bytes!("test-bytes.bin")
    Stdout.line!("Bytes: ${bytes.len().to_str()}")

    # Clean up
    File.delete!("test-file.txt")
    File.delete!("test-bytes.bin")
    Stdout.line!("Cleaned up!")

    Ok({})
}
