(
    (.tasks // {}) |
        to_entries[] |
        select((.value |
            type) == "object") |
        select((.value.shell // "") |
        (startswith("bash") or test("vars\\.task_shell"))) |
        select((.value.run |
            type) == "string") |
        select(.value.run |
        test("\n")) |
        "\(.key) \(.value.run |
            @base64)"
),

# Shell held in a [vars] entry, which a task references instead of spelling the body out.
# Named by the `_sh` suffix: a [vars] value is just a string, so nothing else tells shell apart from the
# Tera-templated paths beside it, and masking one of those would leave shellcheck reading fragments. `$$`
# collapses back to `$` because a [vars] value doubles every dollar to survive mise's config-load expansion,
# and it is the shell's `$` that shellcheck has to see. Each branch is parenthesised because `,` binds tighter
# than `|`, so an unbracketed second stream would run against the first one's output instead of the input.
(
    (.vars // {}) |
        to_entries[] |
        select((.value |
            type) == "string") |
        select(.key |
        test("_sh$")) |
        "vars__\(.key) \(.value |
            split("$$") |
            join("$") |
            @base64)"
)
