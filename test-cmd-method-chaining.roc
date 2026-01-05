app [main!] { pf: platform "./platform/main.roc" }

import pf.Cmd
import pf.Stdout

main! = |_args| {
    # Test method chaining with -> operator
    _cmd1 = Cmd.new("ls")->Cmd.arg("-l")->Cmd.args(["-a", "-h"])

    # Test multiline
    _cmd2 =
        Cmd.new("env")
        ->Cmd.clear_envs
        ->Cmd.env("FOO", "bar")
        ->Cmd.envs([("BAZ", "qux")])

    # Test with effects
    Stdout.line!("Testing Cmd method chaining...")

    # Execute a simple command
    cmd = Cmd.new("echo")->Cmd.args(["Hello"])
    exit_result = Cmd.exec_exit_code!(cmd)
    match exit_result {
        Ok(exit_code) => Stdout.line!("Exit code: ${exit_code.to_str()}")
        Err(_) => Stdout.line!("Error running command")
    }

    Ok({})
}
