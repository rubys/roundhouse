# frozen_string_literal: true
# The key every signed message derives from, when the deployment names
# none: signed and flash cookies, `cookies.signed`, signed ids (campfire's
# sign-in transfer links), signed GlobalIDs, Active Storage ids and Turbo
# stream names.
#
# `SECRET_KEY_BASE` wins when it is set. Otherwise the key is read from
# `storage/secret_key_base`, and generated there on first boot: 64 random
# bytes as hex, the size `bin/rails secret` prints, written owner-only.
#
# Before this, an unset `SECRET_KEY_BASE` signed everything with the empty
# string, a key anyone can derive, and so anyone could forge any signed
# value, including a transfer link for any user. Rails never runs with a
# public key: development generates `tmp/local_secret.txt` the same way, and
# production refuses to boot without a key.
#
# STORAGE, NOT TMP. The emitted binary has no separate production mode to
# make an unset key fatal, and its documented deployment (the campfire
# Docker image) mounts `storage/` as its one persistent volume. There a
# generated key survives a restart, as the database beside it does. In
# `tmp/` every restart would sign everyone out.
#
# Ruby family only; required from both boots before the key is parked.
require "securerandom"

module LocalSecret
  PATH = "storage/secret_key_base"

  def self.resolve(env_value)
    return env_value if !env_value.nil? && !env_value.empty?
    if File.exist?(PATH)
      stored = File.read(PATH).strip
      return stored unless stored.empty?
    end
    key = SecureRandom.hex(64)
    Dir.mkdir("storage") unless Dir.exist?("storage")
    File.write(PATH, key + "\n")
    File.chmod(0600, PATH)
    key
  end
end
