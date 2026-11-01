class ProbeKitRunner
  def self.probe_constant_index
    AliasRecord["name"]
  end
  def self.probe_constant_write
    AliasRecord["name"] = 1
    AliasRecord.last_key
  end
  def self.probe_local_index
    klass = AliasRecord
    klass["name"]
  end
  def self.probe_self_index
    AliasRecord.read_self
  end
  def self.probe_send_index
    AliasRecord.send(:[], "name")
  end
  def self.probe_public_send_index
    AliasRecord.public_send(:[], "name")
  end
  def self.probe___send___index
    AliasRecord.__send__(:[], "name")
  end
  def self.probe_plain_record_index
    AliasRecord.first["name"]
  end
end
