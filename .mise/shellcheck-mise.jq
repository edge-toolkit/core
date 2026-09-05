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
