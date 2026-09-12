# The image processor's swap point — `ActiveStorage::Processor
# .transform`, which `VariantWithRecord#processed` hands the original's
# bytes to. The shared definition (runtime/ruby/active_storage.rb)
# RAISES, and this file leaves it so: an app that declares no
# `attachable.variant` never reaches it and links no image library.
#
# When the app DOES declare variants, `project.rs` swaps this file for
# runtime/spinel/facades/active_storage_processor_vips.rb — the reopen
# over ruby-vips (`require "vips"`: the spinel-ruby-vips spin package
# on the spinel tree, the gem on the CRuby one; same subset surface on
# both) — and the manifest / Gemfile gains the dependency. Same grain
# as the bcrypt façade: whole-file, one require anchor either way.
