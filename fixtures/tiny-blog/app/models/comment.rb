class Comment < ApplicationRecord
  belongs_to :post

  # Rails' grouped count: a Hash of post_id => COUNT, not the scalar
  # `count` answers. Here because nothing else in `fixtures/` groups a
  # query, and the whole path — the `group_count` rename, the type it
  # carries, the Arel `GroupCount` projection, and the Hash hydrate
  # every target emits — had no forcing function until a real app was
  # checked (issues #75-#78). tiny-blog is where the toolchain gates
  # (crystal, python, typescript, go) compile the absent-feature shapes
  # real-blog can't reach.
  def self.counts_by_post
    Comment.group(:post_id).count
  end
end
