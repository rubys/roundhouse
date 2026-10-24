# Extend-only class objects using the default query have a separate native
# respond_to? gap in this combined program. Keep their inference assertions.
p JsonProtocolProbe.nested_false
p JsonProtocolProbe.own_query
puts JsonProtocolProbe.nested_false.class.to_s
puts JsonProtocolProbe.deferred_conflict.class.to_s
puts JsonProtocolProbe.nested_nil.class.to_s
puts JsonProtocolProbe.nested_alias.class.to_s
puts JsonProtocolProbe.nested_alias_method.class.to_s
puts JsonProtocolProbe.nested_writer.class.to_s
puts JsonProtocolProbe.relayed_false.class.to_s
puts JsonProtocolProbe.diamond_false.class.to_s
puts JsonProtocolProbe.inherited_conflict.class.to_s
puts JsonProtocolProbe.include_order_false.class.to_s
puts JsonProtocolProbe.include_order_writer.class.to_s
puts JsonProtocolProbe.own_query.class.to_s
puts JsonProtocolProbe.inherited_query.class.to_s
puts JsonProtocolProbe.included_query.class.to_s
puts JsonProtocolProbe.relayed_query.class.to_s
puts JsonProtocolProbe.alias_query.class.to_s
puts JsonProtocolProbe.alias_method_query.class.to_s
puts JsonProtocolProbe.accept_query.class.to_s
puts JsonProtocolProbe.query_writer.class.to_s
puts JsonProtocolProbe.class_query.class.to_s
puts JsonProtocolProbe.inherited_class_query.class.to_s
puts JsonProtocolProbe.extended_query.class.to_s
puts JsonProtocolProbe.relayed_extended_query.class.to_s
puts JsonProtocolProbe.constant.class.to_s
puts JsonProtocolProbe.returned.class.to_s
puts JsonProtocolProbe.new.ivar.class.to_s
puts JsonProtocolProbe.local_alias.class.to_s
puts JsonProtocolProbe.local_query_alias.class.to_s
puts JsonProtocolProbe.absolute.class.to_s
puts JsonProtocolProbe.options.class.to_s
puts JsonProtocolProbe.nested_dump.class.to_s
puts JsonProtocolProbe.own_false.class.to_s
puts JsonProtocolProbe.own_writer.class.to_s
puts JsonProtocolProbe.inherited_own_writer.class.to_s
puts JsonProtocolProbe.ordinary.class.to_s
puts JsonProtocolProbe.disagreeing_writer.class.to_s
puts JsonProtocolProbe.union(true).class.to_s
puts JsonProtocolProbe.union(false).class.to_s
begin
  JsonProtocolProbe.query_union(true)
rescue NoMethodError => dump_error
  puts dump_error.class.to_s
end
puts JsonProtocolProbe.query_union(false).class.to_s
