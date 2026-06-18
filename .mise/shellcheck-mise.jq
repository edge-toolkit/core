(.tasks // {}) |
    to_entries[] |
    select((.value |
        type) == "object") |
    select((.value.shell // "") |
    startswith("bash")) |
    select((.value.run |
        type) == "string") |
    select(.value.run |
    test("\n")) |
    "\(.key) \(.value.run |
        @base64)"
