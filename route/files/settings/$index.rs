// Bloom also synthesizes authenticated wallet children for account settings.
petal::route_file!(spec: petal::static_dir_spec(), list: vec![petal::writable("api-key"), petal::file("status.json")]);
