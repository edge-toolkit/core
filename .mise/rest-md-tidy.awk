# Rewrites the two constructs in vimanam's Markdown that the Markdown rules reject.
# It writes `<br/>` between the paragraphs of a table cell, and an emphasis-only line where an operation has no
# request or response schema; they become a space and a plain sentence.
{ gsub("<br/>", " ") }
/^[*]No (request|response) schema available[*]$/ { gsub("[*]", ""); $0 = $0 "." }
{ print }
