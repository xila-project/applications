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
        pin = '70b503ba2faa08dcba2936db94cccfe286cd4a76'
        self.assertIn('@' + pin, ci)
        self.assertIn('@' + pin, release)
        self.assertIn('builder-ref: ' + pin, release)
        self.assertNotIn('id-token: write', ci)
        self.assertIn('id-token: write', release)
        self.assertIn('NETBIRD_SETUP_KEY: ${{ secrets.NETBIRD_SETUP_KEY }}', release)

    def test_shared_quality_keeps_app_specific_checks(self):
        ci = (ROOT / '.github/workflows/ci.yml').read_text()
        tasks = manifest('mise.ci.toml')['tasks']
        self.assertIn('xila-project/workspace/.github/workflows/quality.yml@', ci)
        extra = tasks['ci:quality-extra']['run']
        for check in ('validate_manifests.py', 'unittest discover', 'localization:lint'):
            self.assertIn(check, extra)
        self.assertIn('needs: [quality, guest, host, abi, localization, package]', ci)
        for line in ci.splitlines():
            self.assertNotRegex(line, r'^      - (uses|run|env):')
        self.assertIn('name: "Gate | Verify mandatory checks"', ci)

    def test_japanese_checkout_validation_uses_wasi_and_bypasses_mbx(self):
        ci = (ROOT / '.github/workflows/ci.yml').read_text()
        step = ci.split('name: "Test | Japanese with companion Core"', 1)[1].split('  package:', 1)[0]
        self.assertIn('XILA_MBX: "0"', step)
        self.assertIn('CARGO_TARGET_DIR: ${{ runner.temp }}/xila-ja-target', step)
        self.assertIn('cp -a .ci-core-ja "$RUNNER_TEMP/xila-core-ja"', step)
        self.assertIn('cp -a .ci-platforms-ja "$RUNNER_TEMP/xila-platforms-ja"', step)
        self.assertIn('mise run ci:localization-japanese', step)
        task = manifest('mise.ci.toml')['tasks']['ci:localization-japanese']
        self.assertEqual(task['extends'], 'ci:rust-wasi')
        self.assertIn('--target wasm32-wasip1', task['run'])
        self.assertNotIn('xila-application-shortcuts', task['run'])
        self.assertIn('--core "$RUNNER_TEMP/xila-core-ja"', task['run'])

    def test_ci_caches_are_separate_and_use_one_wrapper(self):
        ci = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertIn('mise run ci:packages-host', ci)
        self.assertIn('mise run ci:packages-linker', ci)
        self.assertNotIn('apt-get', ci)
        self.assertIn('v1-apps-host', ci)
        self.assertIn('v1-apps-wasi', ci)
        self.assertIn('v1-apps-localization', ci)
        self.assertIn('cache-mode: objects', ci)
        config = manifest('mise.ci.toml')
        self.assertEqual(config['wrappers']['cargo']['env']['MBX_CARGO_SHIM_MODE'], '1')
        self.assertNotIn('mr-boxington', str(config.get('task_templates', {})))


if __name__ == '__main__':
    unittest.main()
