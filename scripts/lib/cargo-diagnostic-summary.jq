fromjson?
| select(.reason == "compiler-message")
| .message as $diagnostic
| select($diagnostic.level == "error")
| ($diagnostic.spans | map(select(.is_primary)) | first) as $primary_span
| ($diagnostic.spans | first) as $first_span
| ($primary_span // $first_span) as $span
| (
    $diagnostic.children // []
    | map(select(.level == "help") | .message)
    | map(select(. != ""))
    | first
  ) as $help
| [
    (
      if $span then
        "\($span.file_name):\($span.line_start):\($span.column_start)"
      else
        "diagnostic"
      end
      + if $diagnostic.code.code then " \($diagnostic.code.code)" else "" end
    ),
    "  \($diagnostic.message)",
    (if $help then "  help: \($help)" else empty end),
    ""
  ][]
