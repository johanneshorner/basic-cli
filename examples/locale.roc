app [main!] { pf: platform "../platform/main.roc" }

import pf.Stdout
import pf.Locale

# Getting the preferred locale and all available locales

main! = |_args| {
    locale_str = Locale.get!()
    Stdout.line!("The most preferred locale for this system or application: ${locale_str}")

    all_locales = Locale.all!()
    Stdout.line!("All available locales: ${Inspect.to_str(all_locales)}")

    Ok({})
}