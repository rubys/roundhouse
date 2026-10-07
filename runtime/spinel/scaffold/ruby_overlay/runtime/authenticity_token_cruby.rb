# CRuby-only: the masked-token XOR as two 32-byte unpacks and one pack.
#
# The shared AuthenticityToken.xor builds its result one `Integer#chr` at
# a time (32 Strings per token, and as many concatenations) because that
# is what every target can transpile. A page mints a token per form, so
# under CRuby that was a top allocation site on light pages. Same bytes
# out; the String is BINARY, and its consumers (Base64, secure_compare)
# read it as bytes.
module ActionController
  module AuthenticityToken
    def self.xor(a, b)
      x = a.unpack("Q4")
      y = b.unpack("Q4")
      [x[0] ^ y[0], x[1] ^ y[1], x[2] ^ y[2], x[3] ^ y[3]].pack("Q4")
    end
  end
end
