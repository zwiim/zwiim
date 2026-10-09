Pod::Spec.new do |s|
  s.name           = 'ZwiimPreview'
  s.version        = '0.1.0'
  s.summary        = 'In-app file previews in a web view with no app bridge'
  s.description    = 'In-app file previews in a web view with no app bridge'
  s.author         = 'Zwiim'
  s.homepage       = 'https://zwiim.dev'
  s.platforms      = {
    :ios => '16.4'
  }
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  # Swift/Objective-C compatibility
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
  }

  s.source_files = "**/*.{h,m,mm,swift,hpp,cpp}"
end
