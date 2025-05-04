# The default shell config

def create_left_prompt [] {
    # TODO consider changing this depending on environment
    let envname = "bootckit"
    let last_err = (if ($env.LAST_EXIT_CODE) != 0 { $"(ansi red_bold)<($env.LAST_EXIT_CODE)>(ansi reset) " } else { "" })
    $"($last_err)($envname)"
}

$env.PROMPT_COMMAND = {|| create_left_prompt }
$env.config = {
    show_banner: false
}

# For convenience
alias b = bootckit
