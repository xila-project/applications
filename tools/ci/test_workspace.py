"""Application package identity, guest feature and publication policy checks."""
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
GUESTS = ('calculator', 'clock', 'weather', 'pixel_painter', 'text_editor', 'synth')
NATIVE = {'activity': 'xila-activity', 'debug': 'xila-debug-application', 'file_manager': 'xila-file-manager',
          'settings': 'xila-settings', 'terminal': 'xila-terminal', 'package': 'xila-package-application', 'shell/command_line': 'xila-command-line-shell',
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
        self.assertFalse((ROOT / '.github/workflows/release.yml').exists())
        self.assertIn('publish-release: false', ci)
        self.assertIn("sign-artifact: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}", ci)
        self.assertNotIn('NETBIRD_SETUP_KEY:', ci)
        self.assertIn('secrets.XILA_DEV_SIGNING_SEED_B64', ci)
        reference = re.search(r'package-wasm.yml@([0-9a-f]{40})', ci)
        self.assertIsNotNone(reference, "packaging workflow must pin an immutable SHA")
        self.assertIn(f"builder-ref: {reference.group(1)}", ci)
        # Only the release jobs write, and only on pushes to main.
        release = ci.split('\n  release:\n', 1)[1]
        release_job, release_pr_job = release.split('\n  release-pr:\n', 1)
        for body in (release_job, release_pr_job):
            self.assertIn("if: github.event_name == 'push' && github.ref == 'refs/heads/main'", body)
            self.assertIn('contents: write', body)
        self.assertEqual(ci.count('contents: write'), 2)

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

    def test_japanese_uses_the_same_pinned_core_and_wasi_task_as_other_locales(self):
        ci = (ROOT / '.github/workflows/ci.yml').read_text()
        localization = ci.split('  localization:', 1)[1].split('  package:', 1)[0]
        self.assertIn('zh-CN, ja]', localization)
        self.assertIn('run: mise run ci:localization', localization)
        self.assertNotIn('.ci-core-ja', localization)
        self.assertNotIn('.ci-platforms-ja', localization)
        self.assertNotIn('ci:localization-japanese', localization)
        test_step = localization.split('name: "Test | Localization"', 1)[1]
        self.assertNotIn('if: matrix.locale', test_step)
        self.assertNotIn('local_overrides.py', localization)
        core_revision = manifest('Cargo.toml')['workspace']['dependencies']['xila']['rev']
        self.assertIn('ref: ' + core_revision, localization)
        self.assertIn('XILA_JAPANESE_FONT_PATH:', test_step)
        self.assertIn('run: mise run ci:font-japanese', localization)
        task = manifest('mise.ci.toml')['tasks']['ci:localization']
        self.assertIn('--locked --target wasm32-wasip1', task['run'])
        self.assertNotIn('--config', task['run'])

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
