# The default shell config

def create_left_prompt [] {
    let last_err = (if ($env.LAST_EXIT_CODE) != 0 { $"(ansi red_bold)<($env.LAST_EXIT_CODE)>(ansi reset) " } else { "" })
    $"($last_err)bootckit"
}

$env.PROMPT_COMMAND = {|| create_left_prompt }
$env.config = {
    show_banner: false
}


