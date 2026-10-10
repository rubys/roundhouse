# RQRCode — QR-code rendering. campfire's `QrCodeController#show` renders
# a room's join URL as an SVG through it (`RQRCode::QRCode.new(url)
# .as_svg(viewbox: true, fill: :white, color: :black)`), and lobsters
# uses it for 2FA enrollment. This raising façade only stands in where
# the real implementation can't: the spin-shaped spinel tree swaps this
# file for `require "rqrcode"` — the spinel-rqrcode spin package, the
# gem's matrix and SVG over Nayuki's qrcodegen in carried C, byte-
# identical to the gem — whenever the app names RQRCode (see project.rs
# spin_shape). CRuby and JRuby no-op the whole façade chain — the gem is
# pure Ruby and real on both.
#
# Own file rather than a gem_facades.rb section so the swap is
# whole-file — the same grain bcrypt_facade.rb uses.
# rqrcode_core's errors, which an app rescues by name: `QRCodeRunTimeError`
# for data too long to encode, `QRCodeArgumentError` for a bad level or
# size. Defined here so the name resolves where the façade stands; the
# gem and the spin package define the same two.
module RQRCodeCore
  class QRCodeArgumentError < ArgumentError
  end

  class QRCodeRunTimeError < RuntimeError
  end
end

module RQRCode
  class QRCode
    def initialize(_data)
      GemFacade.fail!("RQRCode::QRCode.new")
      @data = _data
    end

    def as_svg(offset: 0, fill: nil, color: nil, module_size: nil, shape_rendering: nil, viewbox: false)
      GemFacade.fail!("RQRCode::QRCode#as_svg")
      ""
    end
  end
end
