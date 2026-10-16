class LocalizedContract < Dry::Validation::Contract
  config.messages.default_locale = :ru
  config.messages.load_paths << "config/locales/contracts.ru.yml"
end

module Users
  class SignupContract < LocalizedContract
    NAMESPACE = "users"
    config.messages.top_namespace = NAMESPACE

    params do
      required(:login).filled(:string, max_size?: 3)
      required(:age).filled(:integer)
      optional(:tags).array(:string)
    end

    rule(:login) do
      key.failure(:taken, who: %w[admin root]) if value == "adm"
      base.failure(:closed) if value == "zzz"
      key.failure(:hashy) if value == "hsh"
    end
  end

  class GuestContract < LocalizedContract
    params do
      required(:login).filled(:string)
    end
  end
end
