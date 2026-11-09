# Rewrites swagger_parser's Dart storage client so the browser can use it.
# swagger_parser renders the application/octet-stream storage endpoints unusably for the browser -- a `dart:io`
# `File` PUT body and a `void` GET response. This makes putFile take a Uint8List (dio sends Uint8List as raw bytes; a
# List<int> would be JSON-encoded) and getFile return the bytes. The END guard fails loudly if a swagger_parser bump
# moves those sites.
/^import / && /dart:io/ { print "import 'dart:typed_data';"; io++; next }
/^  Future<void> getFile\(\{$/ {
  print "  @DioResponseType(ResponseType.bytes)"
  print "  Future<List<int>> getFile({"
  get++
  next
}
/@Body\(\) required File body,/ { sub(/File body/, "Uint8List body"); print; body++; next }
{ print }
END {
  if (io != 1 || get != 1 || body != 1) {
    printf("gen:dart-rest storage fixup miss: io=%d get=%d body=%d\n", io, get, body) > "/dev/stderr"
    exit 1
  }
}
