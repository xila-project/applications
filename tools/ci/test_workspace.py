"""Application package identity, guest feature and publication policy checks."""
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
GUESTS = ('calculator', 'clock', 'weather', 'pixel_painter', 'text_editor', 'synth')
NATIVE = {'activity': 'xila-activity', 'debug': 'xila-debug-application', 'file_manager': 'xila-file-manager',
          'settings': 'xila-settings', 'terminal': 'xila-terminal', 'shell/command_line': 'xila-command-line-shell',
          'shell/graphical': 'xila-graphical-shell'}


def manifest(path):
    return tomllib.loads((ROOT / path).read_text())


class ApplicationPolicy(unittest.TestCase):
    def test_littlefs_configuration_is_owned_by_core(self):
        self.assertFalse((ROOT / 'config/littlefs/little_fs_config.h').exists())
        self.assertNotIn('LITTLEFS_CONFIG', (ROOT / '.cargo/config.toml').read_text())
        binding = manifest('Cargo.toml')['patch']['crates-io']['littlefs2-sys']
        self.assertEqual(binding['git'], 'https://github.com/xila-project/core')
        self.assertRegex(binding['rev'], r'^[0-9a-f]{40}$')

    def test_wasm_executor_and_guest_api_belong_to_applications(self):
        workspace = manifest('Cargo.toml')['workspace']
        self.assertEqual(workspace['dependencies']['wasm']['path'], 'executables/wasm')
        self.assertIn('executables/wasm', workspace['members'])
        wasm = manifest('executables/wasm/Cargo.toml')
        self.assertEqual(wasm['package']['name'], 'xila-wasm')
        self.assertTrue({'host', 'guest', 'memory_32', 'memory_64'} <= set(wasm['features']))
        self.assertIn('dep:wasmi', wasm['features']['host'])
        self.assertNotIn('dep:wasmi', wasm['features']['guest'])

    def test_package_and_binary_names_are_preserved(self):
        for app in GUESTS:
            cargo = manifest(f'executables/{app}/Cargo.toml')
            self.assertEqual(cargo['package']['name'], 'xila-' + app.replace('_', '-'))
            self.assertIn(app, [target['name'] for target in cargo['bin']])
        for path, name in NATIVE.items():
            self.assertEqual(manifest(f'executables/{path}/Cargo.toml')['package']['name'], name)

    def test_wasm_guests_disable_host_defaults(self):
        for app in GUESTS:
            cargo = manifest(f'executables/{app}/Cargo.toml')
            wasm = cargo['target']['cfg(target_arch = "wasm32")']['dependencies']['wasm']
            self.assertFalse(wasm['default-features'])
            self.assertTrue({'guest', 'graphics', 'memory_32'} <= set(wasm['features']))
            self.assertNotIn('host', wasm['features'])

    def test_xila_git_sources_are_immutable_and_platforms_are_dev_only(self):
        workspace = manifest('Cargo.toml')['workspace']['dependencies']
        for key, item in workspace.items():
            if isinstance(item, dict) and item.get('git', '').startswith('https://github.com/xila-project/'):
                self.assertRegex(item['rev'], r'^[0-9a-f]{40}$', key)
        for cargo_path in (ROOT / 'executables').rglob('Cargo.toml'):
            cargo = tomllib.loads(cargo_path.read_text())
            production = [cargo.get('dependencies', {})]
            production.extend(section.get('dependencies', {}) for section in cargo.get('target', {}).values())
            for name in ('testing', 'drivers_native', 'drivers_shared', 'drivers_std'):
                for dependencies in production:
                    self.assertNotIn(name, dependencies, str(cargo_path))

    def test_pr_packaging_never_publishes(self):
        ci = (ROOT / '.github/workflows/ci.yml').read_text()
        release = (ROOT / '.github/workflows/release.yml').read_text()
        self.assertIn('publish-release: false', ci)
        self.assertNotIn('contents: write', ci)
        self.assertIn('publish-release: true', release)
        self.assertIn('tags: ["v*"]', release)
        self.assertIn('XILA_RELEASE_TAG', release)
        self.assertNotIn('pull_request:', release)
        self.assertIn('@71c4da6799a958b188492b0a833bca3550b821ab', ci)

    def test_ci_caches_are_separate_and_use_one_wrapper(self):
        ci = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertIn('v1-apps-host', ci)
        self.assertIn('v1-apps-wasi', ci)
        self.assertIn('v1-apps-localization', ci)
        self.assertIn('github-cache-mode: objects', ci)
        config = manifest('mise.ci.toml')
        self.assertEqual(config['wrappers']['cargo']['env']['MBX_CARGO_SHIM_MODE'], '1')
        self.assertNotIn('mr-boxington', str(config.get('task_templates', {})))


if __name__ == '__main__':
    unittest.main()
