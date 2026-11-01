class AliasRecord < ApplicationRecord
  self.table_name = "probe_records"
  def self.[](key)
    key
  end
  def self.[]=(key, value)
    @last_key = key
  end
  def self.last_key
    @last_key
  end
  def self.read_self
    self["name"]
  end
  def query_class_index
    self.class["name"]
  end
  def query_class_write
    self.class["name"] = 1
    self.class.last_key
  end
  def query_class_local
    klass = self.class
    klass["name"]
  end
  def query_class_local_write
    klass = self.class
    klass["name"] = 1
    klass.last_key
  end
  def model_class
    self.class
  end
  def query_class_helper
    model_class["name"]
  end
  def query_class_helper_write
    model_class["name"] = 1
    model_class.last_key
  end
  def instance_index
    self["name"]
  end
  def instance_write
    self["name"] = "changed"
    self["name"]
  end
  def unknown_index(receiver)
    receiver["name"]
  end
  def unknown_write(receiver)
    receiver["name"] = 1
    receiver.last_key
  end
  def receiver_from_argument(receiver)
    receiver
  end
  def unknown_helper_index(receiver)
    receiver_from_argument(receiver)["name"]
  end
  def unknown_helper_write(receiver)
    receiver_from_argument(receiver)["name"] = 1
    receiver.last_key
  end
end
