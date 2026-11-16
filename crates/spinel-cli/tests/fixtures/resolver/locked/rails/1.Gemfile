source ENV.fetch("SPINEL_RESOLVER_SOURCE")

# What `rails new` writes, less its comments.
gem "rails", "= 8.1.4"
gem "propshaft"
gem "sqlite3", ">= 2.1"
gem "puma", ">= 5.0", "< 8.1"
gem "importmap-rails"
gem "turbo-rails"
gem "stimulus-rails"
gem "jbuilder"
gem "tzinfo-data", platforms: %i[ windows jruby ]
gem "solid_cache"
gem "solid_queue"
gem "solid_cable"
gem "bootsnap", require: false
gem "kamal", require: false
gem "thruster", require: false

group :development, :test do
  gem "debug", platforms: %i[ mri windows ], require: "debug/prelude"
  gem "bundler-audit", require: false
  gem "brakeman", require: false
  gem "rubocop-rails-omakase", require: false
end

group :development do
  gem "web-console"
end

group :test do
  gem "capybara"
  gem "selenium-webdriver"
end

gem "nokogiri", "< 1.19.7"
gem "rack", "< 3.2.8"
