# frozen_string_literal: true
# ActiveRecord::Encryption::Errors — the two classes app code raises
# and rescues. Nothing in this runtime encrypts a column. The classes
# exist so a rescue clause can name them.
#
# Ruby-family home, same as ConnectionNotEstablished: the strict
# targets do not load this file.
module ActiveRecord
  module Encryption
    module Errors
      class Decryption < StandardError
      end

      class Configuration < StandardError
      end
    end
  end
end
