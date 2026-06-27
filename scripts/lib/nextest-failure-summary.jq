def useful_lines:
  split("\n")
  | map(select(. != ""))
  | .[-12:];

fromjson?
| select(.type == "test" and .event == "failed")
| [
    "failed: \(.name)",
    (.stdout // "" | useful_lines[]? | "  \(.)"),
    ""
  ][]
