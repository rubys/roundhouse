//! `ActionText::Attachment#attachable` dereferences an sgid through
//! `ActionText::Attachable.locate`, which is GENERATED per app: one
//! `when "<Model>"` per model that mixes `ActionText::Attachable` in,
//! written into the emitted tree's `global_id_locator.rb` between its
//! markers. And `obj.extend Mod` on an instance — what campfire's test
//! does to make a `Room` attachable for one assertion — is a raise stub
//! naming the construct, so the file it sits in still links on spinel.

use roundhouse::ingest::ingest_app_from_tree;

fn app_with(files: Vec<(&str, &str)>) -> roundhouse::App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :name\n  end\n  create_table :rooms do |t|\n    t.string :name\n  end\nend\n";

#[test]
fn the_locator_is_a_case_over_the_attachable_models() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        // campfire's layout: the marker one level down, in a module
        // nested under the model.
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include Mentionable\nend\n",
        ),
        (
            "app/models/user/mentionable.rb",
            "module User::Mentionable\n  include ActionText::Attachable\n\n  def to_attachable_partial_path\n    \"users/mention\"\n  end\nend\n",
        ),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
    ]);
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, roundhouse::fixtures::real_blog()).expect("spinel tree");
    let locator = files
        .iter()
        .find(|(p, _)| p.ends_with("global_id_locator.rb"))
        .map(|(_, c)| c.clone())
        .expect("global_id_locator.rb in the tree");
    // The include is resolved through the concern; Room, which does
    // not mix the module in, gets no arm.
    assert!(
        locator.contains(
            "    def self.locate(model_name, id)\n      case model_name\n      when \"User\"\n        User.find_by({ id: id })\n      end\n    end\n"
        ),
        "generated locate:\n{locator}"
    );
    assert!(!locator.contains("when \"Room\""), "Room is not attachable:\n{locator}");
}

#[test]
fn an_instance_extend_stubs_the_test_that_reaches_for_it() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("test/fixtures/rooms.yml", "pets:\n  name: Pets\n"),
        (
            "test/models/room_test.rb",
            "require \"test_helper\"\nclass RoomTest < ActiveSupport::TestCase\n  test \"dynamic\" do\n    room = rooms(:pets).tap { |r| r.extend ActionText::Attachable }\n    assert_equal \"Pets\", room.name\n  end\n\n  test \"static\" do\n    assert_equal \"Pets\", rooms(:pets).name\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let report = diags
        .iter()
        .find(|d| d.message.contains("Object#extend"))
        .expect("the extend is reported");
    assert_eq!(
        report.severity,
        roundhouse::diagnostic::Severity::Warning,
        "a warning, so the strict archive build still writes the tree"
    );
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("room_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    // The WHOLE test is the stub — what followed the extend was
    // written against the object it produced — and the test beside
    // it is untouched.
    assert!(
        test.contains("  def test_dynamic\n    raise \"roundhouse: Object#extend not supported (all targets)\"\n  end\n"),
        "the dynamic test should be the raise:\n{test}"
    );
    assert!(!test.contains(".extend"), "no extend survives into the emit:\n{test}");
    assert!(
        test.contains("  def test_static\n    raise \"assert_equal failed\" if \"Pets\" != RoomsFixtures.pets.name\n  end\n"),
        "the static test keeps its body:\n{test}"
    );
}

// campfire's `lib/rails_ext/action_text_attachables.rb`, verbatim: the
// rotated-secret tolerance, as an `on_load` reopen.
const TOLERANT_REOPEN: &str = r#"ActiveSupport.on_load(:action_text_content) do
  class ActionText::Attachment
    class << self
      def from_node(node, attachable = nil)
        new(node, attachable || ActionText::Attachment::OpengraphEmbed.from_node(node) || attachable_from_possibly_expired_sgid(node["sgid"]) || ActionText::Attachable.from_node(node))
      end

      private
        # Our @mentions use ActionText attachments, which are signed. If someone rotates SECRET_KEY_BASE, the existing attachments become invalid.
        # This allows ignoring invalid signatures for User attachments in ActionText.
        ATTACHABLES_PERMITTED_WITH_INVALID_SIGNATURES = %w[ User ]

        def attachable_from_possibly_expired_sgid(sgid)
          if message = sgid&.split("--")&.first
            encoded_message = JSON.parse(decode_base64(message))

            decoded_gid = if data = encoded_message.dig("_rails", "data")
              data
            elsif data = encoded_message.dig("_rails", "message")
              decode_base64(data).match(%r{(gid://campfire/[^/]+/\d+)})&.to_s
            else
              nil
            end

            if model = GlobalID.find(decoded_gid)
              model.model_name.to_s.in?(ATTACHABLES_PERMITTED_WITH_INVALID_SIGNATURES) ? model : nil
            end
          end
        rescue ActiveRecord::RecordNotFound
          nil
        end

        def decode_base64(message)
          Base64.strict_decode64(message)
        rescue => _e
          Base64.urlsafe_decode64(message)
        end
    end
  end
end
"#;

fn locator_of(app: &roundhouse::App) -> String {
    let files = roundhouse::project::spinel_base_files(app, roundhouse::fixtures::real_blog()).expect("spinel tree");
    files
        .iter()
        .find(|(p, _)| p.ends_with("global_id_locator.rb"))
        .map(|(_, c)| c.clone())
        .expect("global_id_locator.rb in the tree")
}

#[test]
fn the_tolerant_from_node_reopen_becomes_the_permitted_list() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/user.rb", "class User < ApplicationRecord\n  include ActionText::Attachable\nend\n"),
        ("lib/rails_ext/action_text_attachables.rb", TOLERANT_REOPEN),
    ]);
    assert_eq!(
        app.attachable_unsigned_models.iter().map(|m| m.as_str().to_string()).collect::<Vec<_>>(),
        vec!["User".to_string()],
        "the %w[] list is the fact the reopen holds"
    );
    // Nothing of the reopen's body becomes a library class: the
    // decode lives in the runtime, once.
    assert!(
        !app.library_classes.iter().any(|c| c.name.0.as_str() == "ActionText::Attachment"),
        "the reopen is read, not carried as a class"
    );
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let locator = locator_of(&app);
    assert!(
        locator.contains("    def self.permitted_without_signature\n      [\"User\"]\n    end\n"),
        "generated list:\n{locator}"
    );
}

#[test]
fn an_app_without_the_reopen_keeps_the_empty_default() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/user.rb", "class User < ApplicationRecord\n  include ActionText::Attachable\nend\n"),
    ]);
    assert!(app.attachable_unsigned_models.is_empty());
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let locator = locator_of(&app);
    assert!(
        locator.contains("    def self.permitted_without_signature\n      []\n    end\n"),
        "the scaffold default stands:\n{locator}"
    );
}

#[test]
fn another_on_load_reopen_is_reported_not_dropped() {
    roundhouse::ingest::survey::activate();
    let app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/rails_ext/blob_ext.rb",
            "ActiveSupport.on_load(:active_storage_blob) do\n  class ActiveStorage::Blob\n    def shout\n      filename.to_s.upcase\n    end\n  end\nend\n",
        ),
        // A `from_node` reopen WITHOUT the envelope fingerprint is
        // some other override, and says so.
        (
            "lib/rails_ext/other_from_node.rb",
            "ActiveSupport.on_load(:action_text_content) do\n  class ActionText::Attachment\n    def self.from_node(node)\n      nil\n    end\n  end\nend\n",
        ),
    ]);
    let gaps = roundhouse::ingest::survey::drain();
    assert!(app.attachable_unsigned_models.is_empty());
    let messages: Vec<String> = gaps.iter().map(|g| format!("{g:?}")).collect();
    assert!(
        messages.iter().any(|m| m.contains("on_load(:active_storage_blob)") && m.contains("ActiveStorage::Blob")),
        "the blob reopen is named: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("on_load(:action_text_content)") && m.contains("ActionText::Attachment")),
        "the unfingerprinted from_node reopen is named: {messages:?}"
    );
}

/// `Content.render_attachment` is generated beside the layout: one arm
/// per attachable model whose `to_attachable_partial_path` names a
/// partial the tree carries, dispatched on the resolved model NAME with
/// the finder and the view module spelled as literals.
#[test]
fn the_attachment_render_is_a_case_over_the_attachables_with_a_partial() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/user.rb", "class User < ApplicationRecord\n  include Mentionable\nend\n"),
        (
            "app/models/user/mentionable.rb",
            "module User::Mentionable\n  include ActionText::Attachable\n\n  def to_attachable_partial_path\n    \"users/mention\"\n  end\nend\n",
        ),
        // attachable through the marker, but names no partial: no arm.
        ("app/models/room.rb", "class Room < ApplicationRecord\n  include ActionText::Attachable\nend\n"),
        ("app/views/users/_mention.html.erb", "<div class=\"mention\"><%= user.name %></div>\n"),
        ("app/views/layouts/action_text/contents/_content.html.erb", "<div class=\"trix-content\"><%= yield %></div>\n"),
    ]);
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, std::path::Path::new("fixtures/real-blog")).expect("spinel tree");
    let runtime = files
        .iter()
        .find(|(p, _)| p.ends_with("runtime/action_text.rb"))
        .map(|(_, c)| c.clone())
        .expect("runtime/action_text.rb in the tree");
    assert!(
        runtime.contains(
            "    def self.render_attachment(attachment)\n      case attachment.resolved_model_name\n      when \"User\"\n        user = User.find_by({ id: attachment.resolved_id })\n        user.nil? ? \"\" : Views::Users.mention(user)\n      else\n        \"\"\n      end\n    end\n"
        ),
        "generated render:\n{runtime}"
    );
    assert!(!runtime.contains("when \"Room\""), "Room names no partial:\n{runtime}");
    assert!(
        runtime.contains("Views::Layouts::ActionText::Contents.content(render_attachments)"),
        "the layout wraps the rendered nodes:\n{runtime}"
    );
}

/// A class the node names by CONTENT TYPE — campfire's `OpengraphEmbed`,
/// `attachable_content_type` + a class-side `from_node` + a partial
/// path — gets the arm ahead of the sgid `case`, its partial's local is
/// its `model_name.element`, and the Attachment's `caption` is
/// delegated back onto it through the `attachment=` slot the arm fills.
#[test]
fn a_content_type_attachable_renders_through_its_own_from_node() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/rails_ext/actiontext_opengraph_embeds.rb",
            "class ActionText::Attachment::OpengraphEmbed\n  include ActiveModel::Model\n\n  OPENGRAPH_EMBED_CONTENT_TYPE = \"application/vnd.actiontext.opengraph-embed\"\n\n  class << self\n    def from_node(node)\n      if node[\"content-type\"] == OPENGRAPH_EMBED_CONTENT_TYPE\n        new(href: node[\"href\"], url: node[\"url\"], filename: node[\"filename\"])\n      end\n    end\n  end\n\n  attr_accessor :href, :url, :filename\n\n  def attachable_content_type\n    OPENGRAPH_EMBED_CONTENT_TYPE\n  end\n\n  def to_partial_path\n    \"action_text/attachables/opengraph_embed\"\n  end\nend\n",
        ),
        (
            "app/views/action_text/attachables/_opengraph_embed.html.erb",
            "<figure><%= link_to_if opengraph_embed.href.present?, truncate(opengraph_embed.filename, length: 20), opengraph_embed.href %><span><%= opengraph_embed.caption %></span></figure>\n",
        ),
        ("app/views/layouts/action_text/contents/_content.html.erb", "<div class=\"trix-content\"><%= yield %></div>\n"),
    ]);
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, std::path::Path::new("fixtures/real-blog")).expect("spinel tree");
    let text = |suffix: &str| {
        files
            .iter()
            .find(|(p, _)| p.ends_with(suffix))
            .map(|(_, c)| c.clone())
            .unwrap_or_else(|| panic!("{suffix} in the tree"))
    };
    let runtime = text("runtime/action_text.rb");
    assert!(
        runtime.contains(
            "    def self.render_attachment(attachment)\n      opengraph_embed = ::ActionText::Attachment::OpengraphEmbed.from_node(attachment)\n      unless opengraph_embed.nil?\n        opengraph_embed.attachment = attachment\n        return Views::ActionText::Attachables.opengraph_embed(opengraph_embed)\n      end\n"
        ),
        "generated content-type arm:\n{runtime}"
    );
    let partial = text("app/views/action_text/attachables/_opengraph_embed.rb");
    assert!(
        partial.contains("def self.opengraph_embed_into(io, opengraph_embed, notice, alert)"),
        "the local is the class's element, not the directory's singular:\n{partial}"
    );
    // `link_to_if` is an `if` over the two `link_to` halves, the label
    // escaped once in each.
    assert!(partial.contains("if ! (opengraph_embed.href || \"\").empty?"), "{partial}");
    assert!(partial.contains("ActionView::ViewHelpers.link_to(ActionView::ViewHelpers.truncate("), "{partial}");
    assert!(!partial.contains("html_escape(ActionView::ViewHelpers.link_to"), "the link is not escaped:\n{partial}");
    let class = text("app/models/action_text/attachment/opengraph_embed.rb");
    assert!(class.contains("  def attachment=(value)\n    @attachment = value\n  end\n"), "{class}");
    assert!(class.contains("  def caption\n    a = @attachment\n"), "{class}");
}
